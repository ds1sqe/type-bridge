//! Dynamic managers backed by runtime descriptors.

use std::sync::Arc;

use crate::_descriptor::{EntityDescriptor, RelationDescriptor};
use crate::_dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicEntityIdentity, DynamicEntityRow, DynamicExpr,
    DynamicRelationIdentity, DynamicRelationRow, DynamicRolePlayerInput, DynamicSort,
};
use crate::_registry::DescriptorRegistry;
use crate::WriteResult;
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::match_request::{HydratedThing, MatchResult, SlotValue};
use crate::plan_builders::BuiltPlan;
use crate::plan_builders::bulk_write::WriteKind;
use crate::plan_builders::manager::{ManagerPlans, ManagerRead, ManagerWrite, ReadTerminal};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::query_plan_exec::PlanAnswer;
use crate::session::backend::AnswerCancellation;
use crate::session::backend::{QueryResult, TxType};
use crate::session::{Database, TransactionContext};

use super::hydration::{
    coalesce_dynamic_relation_by_iid, coalesce_dynamic_relations, extract_count,
    hydrate_dynamic_entity, hydrate_dynamic_entity_identity, hydrate_dynamic_relation_identity,
};
use super::query_builder;

/// CRUD manager for an entity described at runtime.
#[derive(Clone)]
pub struct DynamicEntityManager<'db> {
    target: DynamicExecutionTarget<'db>,
    descriptor: Arc<EntityDescriptor>,
    plans: Option<ManagerPlans>,
}

impl<'db> DynamicEntityManager<'db> {
    /// Create a manager that executes native plans under installed registry authority.
    pub fn from_registry(
        db: &'db Database,
        registry: Arc<DescriptorRegistry>,
        descriptor: Arc<EntityDescriptor>,
    ) -> Self {
        let mut plans = ManagerPlans::new(registry).with_query_limits(db.shared_query_limits());
        if !db.supports_given_stage() {
            plans.transport = crate::plan_builders::ValueTransport::Literal;
        }
        Self {
            target: DynamicExecutionTarget::Database(db),
            descriptor,
            plans: Some(plans),
        }
    }

    /// Bind a registry-backed manager to an existing transaction.
    pub fn with_registry_transaction(
        tx: TransactionContext,
        registry: Arc<DescriptorRegistry>,
        descriptor: Arc<EntityDescriptor>,
    ) -> Self {
        Self {
            plans: Some(ManagerPlans::new(registry).with_query_limits(tx.shared_query_limits())),
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor,
        }
    }

    /// Use the literal transport when the owning database cannot send given rows.
    #[doc(hidden)]
    pub fn literal_values(mut self) -> Self {
        self.plans
            .as_mut()
            .expect("registry-backed manager")
            .transport = crate::plan_builders::ValueTransport::Literal;
        self
    }

    pub(crate) fn in_transaction(&self, tx: TransactionContext) -> Self {
        Self {
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor: Arc::clone(&self.descriptor),
            plans: self.plans.clone(),
        }
    }

    /// Return the descriptor used by this manager.
    pub fn descriptor(&self) -> &Arc<EntityDescriptor> {
        &self.descriptor
    }

    pub(crate) async fn mutate_many_identities(
        &self,
        items: &[(Option<String>, DynamicAttributeMap)],
        kind: WriteKind,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let inputs = items
            .iter()
            .map(|(iid, fields)| (iid.as_deref(), fields, &[][..]))
            .collect::<Vec<_>>();
        let program = plans.write_many(&self.descriptor.type_name, kind, &inputs)?;
        self.target.write_plan_group(program, |_| Ok(())).await
    }

    pub(crate) async fn delete_identity(
        &self,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
    ) -> Result<()> {
        let plans = self.plans.as_ref().ok_or_else(|| {
            crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "This operation requires a registry-backed manager",
            )
        })?;
        let plan = plans.delete_identity(&self.descriptor.type_name, iid, attributes, &[])?;
        self.target.write_plan(&plan, |_| Ok(())).await
    }

    pub(crate) async fn delete_identity_snapshots<T>(
        &self,
        items: &[(Option<String>, DynamicAttributeMap)],
        materialize: impl FnOnce(Vec<DynamicEntityRow>) -> Result<T>,
    ) -> Result<T> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        self.target
            .returning(
                plans,
                &self.descriptor.type_name,
                super::returning::DeleteSelection::Entities(items),
                false,
                |registry, things, _, _| {
                    things
                        .iter()
                        .map(|(_, thing)| plan_entity_row(registry, thing))
                        .collect::<Result<Vec<_>>>()
                        .and_then(materialize)
                },
            )
            .await
    }

    pub(crate) async fn delete_snapshots<T>(
        &self,
        iids: &[String],
        materialize: impl FnOnce(Vec<DynamicEntityRow>) -> Result<T>,
    ) -> Result<T> {
        self.delete_models_returning_mapped(iids, false, materialize)
            .await
    }

    pub(crate) async fn delete_models_returning(
        &self,
        iids: &[String],
        exact: bool,
    ) -> Result<Vec<DynamicEntityRow>> {
        self.delete_models_returning_mapped(iids, exact, Ok).await
    }

    async fn delete_models_returning_mapped<T>(
        &self,
        iids: &[String],
        exact: bool,
        materialize: impl FnOnce(Vec<DynamicEntityRow>) -> Result<T>,
    ) -> Result<T> {
        if iids.is_empty() {
            return materialize(Vec::new());
        }
        validate_delete_iids(iids)?;
        let plans = self.plans.as_ref().ok_or_else(|| {
            crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "Deletion snapshots require a registry-backed manager",
            )
        })?;
        self.target
            .returning(
                plans,
                &self.descriptor.type_name,
                super::returning::DeleteSelection::Iids(iids),
                exact,
                |registry, things, _, _| {
                    things
                        .iter()
                        .map(|(_, thing)| plan_entity_row(registry, thing))
                        .collect::<Result<Vec<_>>>()
                        .and_then(materialize)
                },
            )
            .await
    }

    /// Insert an entity input and return every complete hydrated root.
    pub async fn insert(
        &self,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        self.write_mapped(WriteKind::Insert, None, attributes, Ok)
            .await
    }

    /// Insert multiple dynamic entities in one Rust transaction.
    pub async fn insert_many(&self, items: &[DynamicAttributeMap]) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Insert).await
    }

    /// Put an entity input and return every complete hydrated root.
    pub async fn put(
        &self,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        self.write_mapped(WriteKind::PartialPut, None, attributes, Ok)
            .await
    }

    /// Put multiple dynamic entities in one Rust transaction.
    pub async fn put_many(&self, items: &[DynamicAttributeMap]) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Put).await
    }

    /// Insert or update one exact-type entity without reusing a subtype.
    pub async fn put_exact(
        &self,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        self.write_mapped(WriteKind::Put, None, attributes, Ok)
            .await
    }

    /// Insert or update exact-type entities in input order within one transaction.
    pub async fn put_many_exact(&self, items: &[DynamicAttributeMap]) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        let plans = self.plans.as_ref().expect("registry-backed manager");
        return self.write_many_by_plan(plans, items, WriteKind::Put).await;
    }

    /// Update one dynamic entity's non-key attributes.
    pub async fn update(
        &self,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        self.write_mapped(WriteKind::PartialUpdate, iid, attributes, Ok)
            .await
    }

    /// Completely replace non-key ownership on one exact-type entity identified by a mandatory
    /// canonical IID; omitted optional and multivalue values are removed.
    pub async fn update_exact(
        &self,
        iid: &str,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        crate::_dynamic::validate_exact_iid(&self.descriptor.type_name, iid)?;
        self.write_mapped(WriteKind::Update, Some(iid), attributes, Ok)
            .await
    }

    /// Replace and rehydrate one exact entity in the same write transaction.
    pub async fn update_and_get_exact(
        &self,
        iid: &str,
        attributes: &DynamicAttributeMap,
    ) -> Result<WriteResult<DynamicEntityRow>> {
        self.update_exact(iid, attributes).await
    }

    /// Replace and rehydrate exact entities atomically in input order.
    pub async fn update_many_and_get_exact(
        &self,
        items: &[(String, DynamicAttributeMap)],
    ) -> Result<Vec<DynamicEntityRow>> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = self.in_transaction(tx.clone());
                match manager
                    .update_many_and_get_exact_in_transaction(items)
                    .await
                {
                    Ok(rows) => {
                        commit_manager(&tx, &self.plans).await?;
                        Ok(rows)
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }

            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                self.update_many_and_get_exact_in_transaction(items).await
            }
        }
    }

    /// Fetch entities matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicEntityRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                &equality_expressions(filters),
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_entity_row).await;
        }

        let typeql = query_builder::build_dynamic_entity_fetch(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch only entities whose concrete type exactly matches this descriptor.
    pub async fn get_exact(&self, filters: &[Filter]) -> Result<Vec<DynamicEntityRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                &equality_expressions(filters),
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_entity_row).await;
        }

        let typeql =
            query_builder::build_dynamic_entity_fetch_exact(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT FETCH");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                expressions,
                sorts,
                limit,
                offset,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_entity_row).await;
        }

        let typeql = query_builder::build_dynamic_entity_expr_fetch(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch exact-type entities matching dynamic expressions, sorting, and pagination.
    pub async fn get_exact_with_query(
        &self,
        expressions: &[DynamicExpr],
        sorts: &[DynamicSort],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<DynamicEntityRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                sorts,
                limit,
                offset,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_entity_row).await;
        }

        let typeql = query_builder::build_dynamic_entity_expr_fetch_exact(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT EXPR FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch only the first exact-type entity matching dynamic expressions.
    pub async fn first_exact_with_query(
        &self,
        expressions: &[DynamicExpr],
    ) -> Result<Option<DynamicEntityRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                &[],
                Some(1),
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return Ok(plan_fetch(&self.target, plans, program, plan_entity_row)
                .await?
                .into_iter()
                .next());
        }

        let typeql = query_builder::build_dynamic_entity_expr_fetch_exact(
            &self.descriptor,
            expressions,
            &[],
            Some(1),
            None,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT FIRST");
        // raw remainder: no registry authority; removed in 06
        match self.target.execute(&typeql, TxType::Read).await? {
            QueryResult::Documents(docs) => docs
                .first()
                .map(|doc| hydrate_dynamic_entity(&self.descriptor, doc))
                .transpose(),
            QueryResult::Ok => Ok(None),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from first query, got Rows".into(),
            }),
        }
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
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                &[DynamicExpr::Iid { iid: iid.into() }],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            let rows = plan_fetch(&self.target, plans, program, |registry, thing| {
                if !iid.eq_ignore_ascii_case(thing.concept_id().as_str()) {
                    return Err(snapshot_error(&self.descriptor.type_name));
                }
                plan_entity_row(registry, thing)
            })
            .await?;
            return match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        let typeql = query_builder::build_dynamic_entity_fetch_by_iid(&self.descriptor, iid, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC FETCH BY IID");
        // raw remainder: no registry authority; removed in 06
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

    /// Fetch one exact-type entity by its TypeDB IID.
    pub async fn get_by_iid_exact(&self, iid: &str) -> Result<Option<DynamicEntityRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                &[DynamicExpr::Iid { iid: iid.into() }],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            let rows = plan_fetch(&self.target, plans, program, |registry, thing| {
                if !iid.eq_ignore_ascii_case(thing.concept_id().as_str()) {
                    return Err(snapshot_error(&self.descriptor.type_name));
                }
                plan_entity_row(registry, thing)
            })
            .await?;
            return match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        let typeql =
            query_builder::build_dynamic_entity_fetch_by_iid_exact(&self.descriptor, iid, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT FETCH BY IID");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => match docs.len() {
                0 => Ok(None),
                1 => hydrate_dynamic_entity(&self.descriptor, &docs[0]).map(Some),
                n => Err(OrmError::Hydration {
                    type_name: self.descriptor.type_name.clone(),
                    message: format!("Expected 0 or 1 exact result for IID lookup, got {n}"),
                }),
            },
            QueryResult::Ok => Ok(None),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from exact fetch query, got Rows".into(),
            }),
        }
    }

    /// Fetch all entities for this descriptor.
    pub async fn all(&self) -> Result<Vec<DynamicEntityRow>> {
        self.get(&[]).await
    }

    /// Fetch all exact-type entities for this descriptor.
    pub async fn all_exact(&self) -> Result<Vec<DynamicEntityRow>> {
        self.get_exact(&[]).await
    }

    /// Discover subtype-inclusive entity identities without fetching attributes.
    pub async fn discover_all(&self) -> Result<Vec<DynamicEntityIdentity>> {
        if let Some(plans) = &self.plans {
            let identities =
                plan_identities(&self.target, plans, &self.descriptor.type_name, false, None)
                    .await?;
            let identities = identities
                .into_iter()
                .map(|(iid, type_name)| DynamicEntityIdentity { iid, type_name })
                .collect::<Vec<_>>();
            return Ok(identities);
        }

        let typeql =
            query_builder::build_dynamic_entity_identity_discovery(&self.descriptor, None, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC ENTITY IDENTITY DISCOVERY");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_identity_documents(result)
    }

    /// Discover one subtype-inclusive entity identity by canonical IID.
    pub async fn discover_by_iid(&self, iid: &str) -> Result<Option<DynamicEntityIdentity>> {
        if let Some(plans) = &self.plans {
            let identities = plan_identities(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                Some(iid),
            )
            .await?;
            let mut identities = identities
                .into_iter()
                .map(|(iid, type_name)| DynamicEntityIdentity { iid, type_name })
                .collect::<Vec<_>>();
            return match identities.len() {
                0 => Ok(None),
                1 => Ok(identities.pop()),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        let typeql = query_builder::build_dynamic_entity_identity_discovery(
            &self.descriptor,
            Some(iid),
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC ENTITY IDENTITY DISCOVERY BY IID");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        let mut identities = self.hydrate_identity_documents(result)?;
        match identities.len() {
            0 => Ok(None),
            1 => {
                let identity = identities.pop().unwrap();
                if identity.iid != iid {
                    return Err(OrmError::Hydration {
                        type_name: self.descriptor.type_name.clone(),
                        message: "Entity identity discovery returned the wrong IID".into(),
                    });
                }
                Ok(Some(identity))
            }
            n => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: format!("Expected 0 or 1 identity for IID lookup, got {n}"),
            }),
        }
    }

    /// Count entities for this descriptor.
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Count only entities whose concrete type exactly matches this descriptor.
    pub async fn count_exact(&self) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(&self.target, plans, &self.descriptor.type_name, true, &[])
                .await;
        }

        let typeql = query_builder::build_dynamic_entity_count_exact(&self.descriptor, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count entities matching equality filters.
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                &equality_expressions(filters),
            )
            .await;
        }

        let typeql = query_builder::build_dynamic_entity_count(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count entities matching dynamic expressions.
    pub async fn count_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                expressions,
            )
            .await;
        }

        let typeql =
            query_builder::build_dynamic_entity_expr_count(&self.descriptor, expressions, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count exact-type entities matching dynamic expressions in the database.
    pub async fn count_exact_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                true,
                expressions,
            )
            .await;
        }

        let typeql = query_builder::build_dynamic_entity_expr_count_exact(
            &self.descriptor,
            expressions,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT EXPR COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Test exact-type entity existence with a database-side limit-one query.
    pub async fn exists_exact_with_query(&self, expressions: &[DynamicExpr]) -> Result<bool> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                &[],
                None,
                None,
                &[],
                ReadTerminal::Exists,
            )?;
            return match self.target.read_plan(&program).await?.as_slice() {
                [PlanAnswer::Exists(value)] => Ok(*value),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        let typeql = query_builder::build_dynamic_entity_expr_exists_exact(
            &self.descriptor,
            expressions,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXACT EXPR EXISTS");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_exists(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over entities matching equality filters.
    pub async fn aggregate(
        &self,
        filters: &[Filter],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                &equality_expressions(filters),
                &[],
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_entity_aggregate(
            &self.descriptor,
            filters,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC AGGREGATE");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over entities matching dynamic expressions.
    pub async fn aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        if let Some(plans) = &self.plans {
            let program =
                plans.aggregate(&self.descriptor.type_name, expressions, &[], aggregates)?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_entity_expr_aggregate(
            &self.descriptor,
            expressions,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR AGGREGATE");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                &equality_expressions(filters),
                group_fields,
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_entity_group_by_aggregate(
            &self.descriptor,
            filters,
            group_fields,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC GROUP BY AGGREGATE");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                expressions,
                group_fields,
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_entity_expr_group_by_aggregate(
            &self.descriptor,
            expressions,
            group_fields,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR GROUP BY AGGREGATE");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Delete one entity by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let plan = plans.delete(&self.descriptor.type_name, iid, false, false)?;
        return self.target.write_plan(&plan, Ok).await.map(|_| ());
    }

    /// Delete an exact-type entity by mandatory canonical IID.
    pub async fn delete_by_iid_exact(&self, iid: &str) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let plan = plans.delete(&self.descriptor.type_name, iid, true, false)?;
        return self.target.write_plan(&plan, Ok).await.map(|_| ());
    }

    /// Delete exact-type entities, returning complete pre-delete documents in input order.
    /// Missing IIDs are omitted. All snapshots are captured before any mutation.
    pub async fn delete_many_by_iid_exact_returning(
        &self,
        iids: &[String],
    ) -> Result<Vec<serde_json::Value>> {
        if let Some(plans) = &self.plans {
            if iids.is_empty() {
                return Ok(Vec::new());
            }
            validate_delete_iids(iids)?;
            return self
                .target
                .returning(
                    plans,
                    &self.descriptor.type_name,
                    super::returning::DeleteSelection::Iids(iids),
                    true,
                    |_, things, documents, _| snapshot_documents(things, documents),
                )
                .await;
        }

        // raw remainder: no registry authority; removed in 06
        let templates = query_builder::build_dynamic_entity_delete_returning(&self.descriptor)?;
        self.target
            .delete_captured(&templates, iids, |index, documents| {
                if documents.len() > 1 {
                    return Err(snapshot_error(&self.descriptor.type_name));
                }
                if let Some(document) = documents.first() {
                    let row = hydrate_dynamic_entity(&self.descriptor, document)?;
                    validate_snapshot_identity(
                        &self.descriptor.type_name,
                        &iids[index],
                        row.iid.as_deref(),
                        row.type_name.as_deref(),
                    )?;
                }
                Ok(documents.into_iter().next())
            })
            .await
    }

    /// Delete exact-type entities atomically by canonical IID.
    pub async fn delete_many_by_iid_exact(&self, iids: &[String]) -> Result<()> {
        if iids.is_empty() {
            return Ok(());
        }
        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = self.in_transaction(tx.clone());
                match manager.delete_many_by_iid_exact_in_transaction(iids).await {
                    Ok(()) => commit_manager(&tx, &self.plans).await,
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }

            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                self.delete_many_by_iid_exact_in_transaction(iids).await
            }
        }
    }

    async fn update_many_and_get_exact_in_transaction(
        &self,
        items: &[(String, DynamicAttributeMap)],
    ) -> Result<Vec<DynamicEntityRow>> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let inputs = items
            .iter()
            .map(|(iid, attributes)| (Some(iid.as_str()), attributes, &[][..]))
            .collect::<Vec<_>>();
        let plan = plans.write_many(&self.descriptor.type_name, WriteKind::Update, &inputs)?;
        return self
            .target
            .write_plan_group(plan, |answer| {
                group_write_documents(answer, items.len(), &self.descriptor.type_name)?
                    .iter()
                    .zip(items)
                    .map(|(documents, (iid, _))| {
                        let [document] = documents.as_slice() else {
                            return Err(snapshot_error(&self.descriptor.type_name));
                        };
                        let row = hydrate_dynamic_entity(&self.descriptor, document)?;
                        if row.iid.as_deref() != Some(iid) {
                            return Err(snapshot_error(&self.descriptor.type_name));
                        }
                        Ok(row)
                    })
                    .collect()
            })
            .await;
    }

    async fn delete_many_by_iid_exact_in_transaction(&self, iids: &[String]) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        if iids.is_empty() {
            return Ok(());
        }
        let plan = plans.delete_many(&self.descriptor.type_name, iids, true, false)?;
        return self.target.write_plan(&plan, |_| Ok(())).await;
    }

    async fn write_many_by_plan(
        &self,
        plans: &ManagerPlans,
        items: &[DynamicAttributeMap],
        kind: WriteKind,
    ) -> Result<Vec<String>> {
        let inputs = items
            .iter()
            .map(|attributes| (None, attributes, &[][..]))
            .collect::<Vec<_>>();
        let plan = plans.write_many(&self.descriptor.type_name, kind, &inputs)?;
        let layout = plan.identity_layout();
        self.target
            .write_plan_group(plan, |answer| {
                if layout.is_some()
                    || answer
                        .iter()
                        .all(|answer| matches!(answer, PlanAnswer::Rows(_)))
                {
                    return group_write_identity_rows(
                        answer,
                        items.len(),
                        layout,
                        &self.descriptor.type_name,
                    );
                }
                group_write_documents(answer, items.len(), &self.descriptor.type_name)?
                    .iter()
                    .map(|documents| {
                        let [document] = documents.as_slice() else {
                            return Err(snapshot_error(&self.descriptor.type_name));
                        };
                        hydrate_dynamic_entity(&self.descriptor, document)?
                            .iid
                            .ok_or_else(|| snapshot_error(&self.descriptor.type_name))
                    })
                    .collect()
            })
            .await
    }

    async fn write_many(
        &self,
        items: &[DynamicAttributeMap],
        operation: DynamicWriteOperation,
    ) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        let plans = self.plans.as_ref().expect("registry-backed manager");
        let kind = match operation {
            DynamicWriteOperation::Insert => WriteKind::Insert,
            DynamicWriteOperation::Put => WriteKind::PartialPut,
        };
        return self.write_many_by_plan(plans, items, kind).await;
    }

    pub(crate) async fn write_mapped<T>(
        &self,
        kind: WriteKind,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        materialize: impl FnOnce(WriteResult<DynamicEntityRow>) -> Result<T>,
    ) -> Result<T> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let program =
            plans.single_write_program(&self.descriptor.type_name, kind, iid, attributes, &[])?;
        let hydrate = |plan: &BuiltPlan, answer| {
            hydrate_write_rows(
                plans,
                plan,
                &self.descriptor.type_name,
                answer,
                self.target.answer_limits(),
                iid,
                plan_entity_row,
            )
        };
        match program {
            ManagerWrite::One(plan) => {
                self.target
                    .write_plan(&plan, |answer| hydrate(&plan, answer).and_then(materialize))
                    .await
            }
            ManagerWrite::Pieces(program) => {
                self.target
                    .write_plan_group_mapped(
                        program,
                        |plan, answer| hydrate(plan, answer).map(WriteResult::into_items),
                        |groups| {
                            materialize(WriteResult::new(groups.into_iter().flatten().collect()))
                        },
                    )
                    .await
            }
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

    fn hydrate_identity_documents(
        &self,
        result: QueryResult,
    ) -> Result<Vec<DynamicEntityIdentity>> {
        match result {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_entity_identity(&self.descriptor.type_name, doc))
                .collect(),
            QueryResult::Ok => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from identity discovery, got Ok".into(),
            }),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from identity discovery, got Rows".into(),
            }),
        }
    }
}

/// CRUD manager for a relation described at runtime.
#[derive(Clone)]
pub struct DynamicRelationManager<'db> {
    target: DynamicExecutionTarget<'db>,
    descriptor: Arc<RelationDescriptor>,
    plans: Option<ManagerPlans>,
}

impl<'db> DynamicRelationManager<'db> {
    /// Create a manager that executes native plans under installed registry authority.
    pub fn from_registry(
        db: &'db Database,
        registry: Arc<DescriptorRegistry>,
        descriptor: Arc<RelationDescriptor>,
    ) -> Self {
        let mut plans = ManagerPlans::new(registry).with_query_limits(db.shared_query_limits());
        if !db.supports_given_stage() {
            plans.transport = crate::plan_builders::ValueTransport::Literal;
        }
        Self {
            target: DynamicExecutionTarget::Database(db),
            descriptor,
            plans: Some(plans),
        }
    }

    /// Bind a registry-backed manager to an existing transaction.
    pub fn with_registry_transaction(
        tx: TransactionContext,
        registry: Arc<DescriptorRegistry>,
        descriptor: Arc<RelationDescriptor>,
    ) -> Self {
        Self {
            plans: Some(ManagerPlans::new(registry).with_query_limits(tx.shared_query_limits())),
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor,
        }
    }

    /// Use the literal transport when the owning database cannot send given rows.
    #[doc(hidden)]
    pub fn literal_values(mut self) -> Self {
        self.plans
            .as_mut()
            .expect("registry-backed manager")
            .transport = crate::plan_builders::ValueTransport::Literal;
        self
    }

    pub(crate) fn in_transaction(&self, tx: TransactionContext) -> Self {
        Self {
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor: Arc::clone(&self.descriptor),
            plans: self.plans.clone(),
        }
    }

    /// Return the descriptor used by this manager.
    pub fn descriptor(&self) -> &Arc<RelationDescriptor> {
        &self.descriptor
    }

    pub(crate) async fn delete_many_identities(
        &self,
        items: &[(
            Option<String>,
            DynamicAttributeMap,
            Vec<DynamicRolePlayerInput>,
        )],
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let inputs = items
            .iter()
            .map(|(iid, fields, players)| {
                (
                    iid.as_deref(),
                    fields,
                    if iid.is_some() {
                        &[][..]
                    } else {
                        players.as_slice()
                    },
                )
            })
            .collect::<Vec<_>>();
        let program = plans.write_many(&self.descriptor.type_name, WriteKind::Delete, &inputs)?;
        self.target.write_plan_group(program, |_| Ok(())).await
    }

    pub(crate) async fn delete_identity(
        &self,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        players: &[DynamicRolePlayerInput],
    ) -> Result<()> {
        let plans = self.plans.as_ref().ok_or_else(|| {
            crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "This operation requires a registry-backed manager",
            )
        })?;
        let plan = plans.delete_identity(&self.descriptor.type_name, iid, attributes, players)?;
        self.target.write_plan(&plan, |_| Ok(())).await
    }

    pub(crate) async fn delete_identity_snapshots<T>(
        &self,
        items: &[(
            Option<String>,
            DynamicAttributeMap,
            Vec<DynamicRolePlayerInput>,
        )],
        materialize: impl FnOnce(Vec<DynamicRelationRow>) -> Result<T>,
    ) -> Result<T> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        self.target
            .returning(
                plans,
                &self.descriptor.type_name,
                super::returning::DeleteSelection::Relations(items),
                false,
                |registry, things, _, _| {
                    things
                        .iter()
                        .map(|(_, thing)| plan_relation_row(registry, thing))
                        .collect::<Result<Vec<_>>>()
                        .and_then(materialize)
                },
            )
            .await
    }

    pub(crate) async fn delete_snapshots<T>(
        &self,
        iids: &[String],
        materialize: impl FnOnce(Vec<DynamicRelationRow>) -> Result<T>,
    ) -> Result<T> {
        self.delete_models_returning_mapped(iids, false, materialize)
            .await
    }

    pub(crate) async fn delete_models_returning(
        &self,
        iids: &[String],
        exact: bool,
    ) -> Result<Vec<DynamicRelationRow>> {
        self.delete_models_returning_mapped(iids, exact, Ok).await
    }

    async fn delete_models_returning_mapped<T>(
        &self,
        iids: &[String],
        exact: bool,
        materialize: impl FnOnce(Vec<DynamicRelationRow>) -> Result<T>,
    ) -> Result<T> {
        if iids.is_empty() {
            return materialize(Vec::new());
        }
        validate_delete_iids(iids)?;
        let plans = self.plans.as_ref().ok_or_else(|| {
            crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "Deletion snapshots require a registry-backed manager",
            )
        })?;
        self.target
            .returning(
                plans,
                &self.descriptor.type_name,
                super::returning::DeleteSelection::Iids(iids),
                exact,
                |registry, things, _, _| {
                    things
                        .iter()
                        .map(|(_, thing)| plan_relation_row(registry, thing))
                        .collect::<Result<Vec<_>>>()
                        .and_then(materialize)
                },
            )
            .await
    }

    /// Insert a relation input and return every complete hydrated root.
    pub async fn insert(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<WriteResult<DynamicRelationRow>> {
        self.write_mapped(WriteKind::Insert, None, attributes, role_players, Ok)
            .await
    }

    /// Insert multiple dynamic relations in one Rust transaction.
    pub async fn insert_many(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Insert).await
    }

    /// Put a relation input and return every complete hydrated root.
    pub async fn put(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<WriteResult<DynamicRelationRow>> {
        self.write_mapped(WriteKind::PartialPut, None, attributes, role_players, Ok)
            .await
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
    ) -> Result<WriteResult<DynamicRelationRow>> {
        self.write_mapped(WriteKind::PartialUpdate, iid, attributes, role_players, Ok)
            .await
    }

    /// Replace the exact relation's owned state while preserving its IID.
    /// The final complete player set must be nonempty because TypeDB removes
    /// roleless relations at commit.
    pub async fn update_exact(
        &self,
        iid: &str,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<WriteResult<DynamicRelationRow>> {
        validate_relation_exact_update_input(&self.descriptor, iid, attributes, role_players)?;
        self.write_mapped(WriteKind::Update, Some(iid), attributes, role_players, Ok)
            .await
    }

    /// Replace and rehydrate one exact relation in the same write transaction.
    pub async fn update_and_get_exact(
        &self,
        iid: &str,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<WriteResult<DynamicRelationRow>> {
        self.update_exact(iid, attributes, role_players).await
    }

    /// Replace and rehydrate exact relations atomically in input order.
    pub async fn update_many_and_get_exact(
        &self,
        items: &[(String, DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<DynamicRelationRow>> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        for (iid, attributes, role_players) in items {
            validate_relation_exact_write_input(
                &self.descriptor,
                Some(iid),
                attributes,
                role_players,
                ExactWritePolicy::Update,
                true,
            )?;
        }
        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = self.in_transaction(tx.clone());
                match manager
                    .update_many_and_get_exact_in_transaction(items)
                    .await
                {
                    Ok(rows) => {
                        commit_manager(&tx, &self.plans).await?;
                        Ok(rows)
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }

            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                self.update_many_and_get_exact_in_transaction(items).await
            }
        }
    }

    /// Put one exact concrete relation.
    ///
    /// Declared keys identify at most one relation. A hit preserves its IID
    /// and keys while replacing non-key and player state; a miss inserts it.
    /// Every complete hydrated relation is returned in answer order.
    pub async fn put_exact(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<WriteResult<DynamicRelationRow>> {
        validate_relation_exact_put_input(&self.descriptor, attributes, role_players)?;
        self.write_mapped(WriteKind::Put, None, attributes, role_players, Ok)
            .await
    }

    /// Put complete exact concrete relations in input order, atomically within
    /// one owned or reused write transaction, returning canonical IIDs in order.
    pub async fn put_many_exact(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<String>> {
        for (attributes, role_players) in items {
            validate_relation_exact_write_input(
                &self.descriptor,
                None,
                attributes,
                role_players,
                ExactWritePolicy::Put,
                true,
            )?;
        }
        if items.is_empty() {
            return Ok(Vec::new());
        }
        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = self.in_transaction(tx.clone());
                match manager.put_many_exact_in_transaction(items).await {
                    Ok(iids) => {
                        commit_manager(&tx, &self.plans).await?;
                        Ok(iids)
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }

            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                self.put_many_exact_in_transaction(items).await
            }
        }
    }

    async fn put_many_exact_in_transaction(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<String>> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        return self.write_many_by_plan(plans, items, WriteKind::Put).await;
    }

    async fn update_many_and_get_exact_in_transaction(
        &self,
        items: &[(String, DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<DynamicRelationRow>> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let inputs = items
            .iter()
            .map(|(iid, attributes, players)| (Some(iid.as_str()), attributes, players.as_slice()))
            .collect::<Vec<_>>();
        let plan = plans.write_many(&self.descriptor.type_name, WriteKind::Update, &inputs)?;
        return self
            .target
            .write_plan_group(plan, |answer| {
                group_write_documents(answer, items.len(), &self.descriptor.type_name)?
                    .iter()
                    .zip(items)
                    .map(|(documents, (iid, _, players))| {
                        let rows =
                            coalesce_dynamic_relation_by_iid(&self.descriptor, documents, iid)?;
                        let [row] = rows.as_slice() else {
                            return Err(snapshot_error(&self.descriptor.type_name));
                        };
                        validate_written_players(
                            &plans.registry,
                            true,
                            &self.descriptor.type_name,
                            row,
                            players,
                        )?;
                        Ok(row.clone())
                    })
                    .collect()
            })
            .await;
    }

    /// Fetch relations matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                &equality_expressions(filters),
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_relation_row).await;
        }

        let typeql = query_builder::build_dynamic_relation_fetch(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch only relations whose concrete type exactly matches this descriptor.
    pub async fn get_exact(&self, filters: &[Filter]) -> Result<Vec<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                &equality_expressions(filters),
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_relation_row).await;
        }

        let typeql =
            query_builder::build_dynamic_relation_fetch_exact(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT FETCH");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                expressions,
                sorts,
                limit,
                offset,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_relation_row).await;
        }

        let typeql = query_builder::build_dynamic_relation_expr_fetch(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch exact-type relations matching dynamic expressions, sorting, and pagination.
    pub async fn get_exact_with_query(
        &self,
        expressions: &[DynamicExpr],
        sorts: &[DynamicSort],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                sorts,
                limit,
                offset,
                &[],
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_relation_row).await;
        }

        let typeql = query_builder::build_dynamic_relation_expr_fetch_exact(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT EXPR FETCH");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch only the first exact-type relation matching dynamic expressions.
    pub async fn first_exact_with_query(
        &self,
        expressions: &[DynamicExpr],
    ) -> Result<Option<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                &[],
                Some(1),
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            return Ok(plan_fetch(&self.target, plans, program, plan_relation_row)
                .await?
                .into_iter()
                .next());
        }

        let typeql = query_builder::build_dynamic_relation_expr_fetch_exact(
            &self.descriptor,
            expressions,
            &[],
            Some(1),
            None,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT FIRST");
        // raw remainder: no registry authority; removed in 06
        match self.target.execute(&typeql, TxType::Read).await? {
            QueryResult::Documents(docs) => {
                let first = docs.first().into_iter().cloned().collect::<Vec<_>>();
                coalesce_dynamic_relations(&self.descriptor, &first)
                    .map(|rows| rows.into_iter().next())
            }
            QueryResult::Ok => Ok(None),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from first query, got Rows".into(),
            }),
        }
    }

    /// Fetch relations matching attribute and role-player filters.
    pub async fn get_with_role_filters(
        &self,
        filters: &[Filter],
        role_filters: &[DynamicRolePlayerInput],
    ) -> Result<Vec<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                &equality_expressions(filters),
                &[],
                None,
                None,
                role_filters,
                ReadTerminal::Fetch,
            )?;
            return plan_fetch(&self.target, plans, program, plan_relation_row).await;
        }

        let typeql = query_builder::build_dynamic_relation_fetch_with_role_filters(
            &self.descriptor,
            filters,
            role_filters,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH WITH ROLE FILTERS");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                false,
                &[DynamicExpr::Iid { iid: iid.into() }],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            let rows = plan_fetch(&self.target, plans, program, |registry, thing| {
                if !iid.eq_ignore_ascii_case(thing.concept_id().as_str()) {
                    return Err(snapshot_error(&self.descriptor.type_name));
                }
                plan_relation_row(registry, thing)
            })
            .await?;
            return Ok(rows);
        }

        let typeql =
            query_builder::build_dynamic_relation_fetch_by_iid(&self.descriptor, iid, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH BY IID");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => {
                coalesce_dynamic_relation_by_iid(&self.descriptor, &docs, iid)
            }
            other => self.hydrate_documents(other),
        }
    }

    /// Fetch exact-type relation rows by TypeDB IID.
    pub async fn get_by_iid_exact(&self, iid: &str) -> Result<Vec<DynamicRelationRow>> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                &[DynamicExpr::Iid { iid: iid.into() }],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )?;
            let rows = plan_fetch(&self.target, plans, program, |registry, thing| {
                if !iid.eq_ignore_ascii_case(thing.concept_id().as_str()) {
                    return Err(snapshot_error(&self.descriptor.type_name));
                }
                plan_relation_row(registry, thing)
            })
            .await?;
            return Ok(rows);
        }

        let typeql =
            query_builder::build_dynamic_relation_fetch_by_iid_exact(&self.descriptor, iid, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT FETCH BY IID");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => {
                coalesce_dynamic_relation_by_iid(&self.descriptor, &docs, iid)
            }
            other => self.hydrate_documents(other),
        }
    }

    /// Fetch all relations for this descriptor.
    pub async fn all(&self) -> Result<Vec<DynamicRelationRow>> {
        self.get(&[]).await
    }

    /// Fetch all exact-type relations for this descriptor.
    pub async fn all_exact(&self) -> Result<Vec<DynamicRelationRow>> {
        self.get_exact(&[]).await
    }

    /// Discover concrete relation identities in database result order.
    pub async fn discover_all(&self) -> Result<Vec<DynamicRelationIdentity>> {
        if let Some(plans) = &self.plans {
            let identities =
                plan_identities(&self.target, plans, &self.descriptor.type_name, false, None)
                    .await?;
            let identities = identities
                .into_iter()
                .map(|(iid, type_name)| DynamicRelationIdentity { iid, type_name })
                .collect::<Vec<_>>();
            return Ok(identities);
        }

        let q =
            query_builder::build_dynamic_relation_identity_discovery(&self.descriptor, None, "$r")?;
        // raw remainder: no registry authority; removed in 06
        match self.target.execute(&q, TxType::Read).await? {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_relation_identity(&self.descriptor.type_name, doc))
                .collect(),
            QueryResult::Ok => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from relation identity discovery, got Ok".into(),
            }),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from relation identity discovery, got Rows".into(),
            }),
        }
    }

    /// Discover at most one concrete relation identity by canonical IID.
    pub async fn discover_by_iid(&self, iid: &str) -> Result<Option<DynamicRelationIdentity>> {
        if let Some(plans) = &self.plans {
            let identities = plan_identities(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                Some(iid),
            )
            .await?;
            let mut identities = identities
                .into_iter()
                .map(|(iid, type_name)| DynamicRelationIdentity { iid, type_name })
                .collect::<Vec<_>>();
            return match identities.len() {
                0 => Ok(None),
                1 => Ok(identities.pop()),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        crate::_dynamic::validate_relation_iid(&self.descriptor.type_name, iid)?;
        let q = query_builder::build_dynamic_relation_identity_discovery(
            &self.descriptor,
            Some(iid),
            "$r",
        )?;
        // raw remainder: no registry authority; removed in 06
        let rows = match self.target.execute(&q, TxType::Read).await? {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_relation_identity(&self.descriptor.type_name, doc))
                .collect::<Result<Vec<_>>>()?,
            QueryResult::Ok => {
                return Err(OrmError::Hydration {
                    type_name: self.descriptor.type_name.clone(),
                    message: "Expected Documents from relation identity discovery, got Ok".into(),
                });
            }
            QueryResult::Rows(_) => {
                return Err(OrmError::Hydration {
                    type_name: self.descriptor.type_name.clone(),
                    message: "Expected Documents from relation identity discovery, got Rows".into(),
                });
            }
        };
        match rows.as_slice() {
            [] => Ok(None),
            [row] if row.iid == iid => Ok(Some(row.clone())),
            [_row] => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Relation identity discovery returned the wrong IID".into(),
            }),
            _ => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: format!(
                    "Expected 0 or 1 relation identity for IID lookup, got {}",
                    rows.len()
                ),
            }),
        }
    }

    /// Count only exact instances of this concrete relation type.
    pub async fn count_exact(&self) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(&self.target, plans, &self.descriptor.type_name, true, &[])
                .await;
        }

        let q = query_builder::build_dynamic_relation_count_exact(&self.descriptor, "$r")?;
        // raw remainder: no registry authority; removed in 06
        extract_count(&self.target.execute(&q, TxType::Read).await?)
    }

    /// Delete one exact concrete relation instance by canonical IID.
    pub async fn delete_by_iid_exact(&self, iid: &str) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let plan = plans.delete(&self.descriptor.type_name, iid, true, false)?;
        return self.target.write_plan(&plan, Ok).await.map(|_| ());
    }

    /// Delete exact-type relations, returning complete pre-delete documents in input order.
    /// Missing IIDs are omitted. All snapshots are captured before any mutation.
    pub async fn delete_many_by_iid_exact_returning(
        &self,
        iids: &[String],
    ) -> Result<Vec<serde_json::Value>> {
        if let Some(plans) = &self.plans {
            if iids.is_empty() {
                return Ok(Vec::new());
            }
            validate_delete_iids(iids)?;
            return self.target.returning(plans, &self.descriptor.type_name, super::returning::DeleteSelection::Iids(iids), true, |registry, things, documents, _| {
                let mut documents = snapshot_documents(things, documents)?;
                for (document, (_, thing)) in documents.iter_mut().zip(things) {
                    let row = plan_relation_row(registry, thing)?;
                    let object = document.as_object_mut().ok_or_else(|| snapshot_error(&self.descriptor.type_name))?;
                    object.retain(|key, _| !key.starts_with("_role_"));
                    object.insert("role_players".into(), row.role_players.into_iter().map(|player| serde_json::json!({"role": player.role_name, "iid": player.player_iid, "type_name": player.player_type_name, "attributes": player.attributes.into_iter().collect::<serde_json::Map<_, _>>() })).collect());
                }
                Ok(documents)
            }).await;
        }

        let mut capture_descriptor = (*self.descriptor).clone();
        for role in &mut capture_descriptor.roles {
            role.cardinality = role.cardinality.map(|(_, maximum)| (0, maximum));
        }
        let templates =
            query_builder::build_dynamic_relation_delete_returning(&capture_descriptor)?;
        // raw remainder: no registry authority; removed in 06
        self.target
            .delete_captured(&templates, iids, |index, documents| {
                relation_snapshot_document(&self.descriptor, &iids[index], documents)
            })
            .await
    }

    /// Delete exact-type relations atomically by canonical IID.
    pub async fn delete_many_by_iid_exact(&self, iids: &[String]) -> Result<()> {
        if iids.is_empty() {
            return Ok(());
        }
        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = self.in_transaction(tx.clone());
                match manager.delete_many_by_iid_exact_in_transaction(iids).await {
                    Ok(()) => commit_manager(&tx, &self.plans).await,
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }

            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                self.delete_many_by_iid_exact_in_transaction(iids).await
            }
        }
    }

    async fn delete_many_by_iid_exact_in_transaction(&self, iids: &[String]) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        if iids.is_empty() {
            return Ok(());
        }
        let plan = plans.delete_many(&self.descriptor.type_name, iids, true, false)?;
        return self.target.write_plan(&plan, |_| Ok(())).await;
    }

    /// Count relations for this descriptor.
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Count relations matching equality filters.
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                &equality_expressions(filters),
            )
            .await;
        }

        let typeql = query_builder::build_dynamic_relation_count(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count relations matching dynamic expressions.
    pub async fn count_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                false,
                expressions,
            )
            .await;
        }

        let typeql =
            query_builder::build_dynamic_relation_expr_count(&self.descriptor, expressions, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count exact-type relations matching dynamic expressions in the database.
    pub async fn count_exact_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        if let Some(plans) = &self.plans {
            return plan_read_count(
                &self.target,
                plans,
                &self.descriptor.type_name,
                true,
                expressions,
            )
            .await;
        }

        let typeql = query_builder::build_dynamic_relation_expr_count_exact(
            &self.descriptor,
            expressions,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT EXPR COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Test exact-type relation existence with a database-side limit-one query.
    pub async fn exists_exact_with_query(&self, expressions: &[DynamicExpr]) -> Result<bool> {
        if let Some(plans) = &self.plans {
            let program = plans.read(
                &self.descriptor.type_name,
                true,
                expressions,
                &[],
                None,
                None,
                &[],
                ReadTerminal::Exists,
            )?;
            return match self.target.read_plan(&program).await?.as_slice() {
                [PlanAnswer::Exists(value)] => Ok(*value),
                _ => Err(snapshot_error(&self.descriptor.type_name)),
            };
        }

        let typeql = query_builder::build_dynamic_relation_expr_exists_exact(
            &self.descriptor,
            expressions,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXACT EXPR EXISTS");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_exists(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over relations matching equality filters.
    pub async fn aggregate(
        &self,
        filters: &[Filter],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                &equality_expressions(filters),
                &[],
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_relation_aggregate(
            &self.descriptor,
            filters,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION AGGREGATE");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over relations matching dynamic expressions.
    pub async fn aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        if let Some(plans) = &self.plans {
            let program =
                plans.aggregate(&self.descriptor.type_name, expressions, &[], aggregates)?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_relation_expr_aggregate(
            &self.descriptor,
            expressions,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR AGGREGATE");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                &equality_expressions(filters),
                group_fields,
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_relation_group_by_aggregate(
            &self.descriptor,
            filters,
            group_fields,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION GROUP BY AGGREGATE");
        // raw remainder: no registry authority; removed in 06
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
        if let Some(plans) = &self.plans {
            let program = plans.aggregate(
                &self.descriptor.type_name,
                expressions,
                group_fields,
                aggregates,
            )?;
            return plan_aggregate(&self.target, program).await;
        }

        let typeql = query_builder::build_dynamic_relation_expr_group_by_aggregate(
            &self.descriptor,
            expressions,
            group_fields,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR GROUP BY AGGREGATE");
        // raw remainder: no registry authority; removed in 06
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Delete one relation by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let plan = plans.delete(&self.descriptor.type_name, iid, false, false)?;
        return self.target.write_plan(&plan, Ok).await.map(|_| ());
    }

    async fn write_many_by_plan(
        &self,
        plans: &ManagerPlans,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
        kind: WriteKind,
    ) -> Result<Vec<String>> {
        let inputs = items
            .iter()
            .map(|(attributes, players)| (None, attributes, players.as_slice()))
            .collect::<Vec<_>>();
        let plan = plans.write_many(&self.descriptor.type_name, kind, &inputs)?;
        let layout = plan.identity_layout();
        self.target
            .write_plan_group(plan, |answer| {
                if layout.is_some()
                    || answer
                        .iter()
                        .all(|answer| matches!(answer, PlanAnswer::Rows(_)))
                {
                    return group_write_identity_rows(
                        answer,
                        items.len(),
                        layout,
                        &self.descriptor.type_name,
                    );
                }
                group_write_documents(answer, items.len(), &self.descriptor.type_name)?
                    .iter()
                    .zip(items)
                    .map(|(documents, (_, players))| {
                        let rows = coalesce_dynamic_relations(&self.descriptor, documents)?;
                        let [row] = rows.as_slice() else {
                            return Err(snapshot_error(&self.descriptor.type_name));
                        };
                        validate_written_players(
                            &plans.registry,
                            kind != WriteKind::Insert,
                            &self.descriptor.type_name,
                            row,
                            players,
                        )?;
                        row.iid
                            .clone()
                            .ok_or_else(|| snapshot_error(&self.descriptor.type_name))
                    })
                    .collect()
            })
            .await
    }

    async fn write_many(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
        operation: DynamicWriteOperation,
    ) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        let plans = self.plans.as_ref().expect("registry-backed manager");
        let kind = match operation {
            DynamicWriteOperation::Insert => WriteKind::Insert,
            DynamicWriteOperation::Put => WriteKind::MatchPut,
        };
        return self.write_many_by_plan(plans, items, kind).await;
    }

    pub(crate) async fn write_mapped<T>(
        &self,
        kind: WriteKind,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
        materialize: impl FnOnce(WriteResult<DynamicRelationRow>) -> Result<T>,
    ) -> Result<T> {
        let plans = self.plans.as_ref().expect("registry-backed manager");
        let program = plans.single_write_program(
            &self.descriptor.type_name,
            kind,
            iid,
            attributes,
            role_players,
        )?;
        let hydrate = |plan: &BuiltPlan, answer| {
            hydrate_write_rows(
                plans,
                plan,
                &self.descriptor.type_name,
                answer,
                self.target.answer_limits(),
                iid,
                plan_relation_row,
            )
        };
        match program {
            ManagerWrite::One(plan) => {
                self.target
                    .write_plan(&plan, |answer| hydrate(&plan, answer).and_then(materialize))
                    .await
            }
            ManagerWrite::Pieces(program) => {
                self.target
                    .write_plan_group_mapped(
                        program,
                        |plan, answer| hydrate(plan, answer).map(WriteResult::into_items),
                        |groups| {
                            materialize(WriteResult::new(groups.into_iter().flatten().collect()))
                        },
                    )
                    .await
            }
        }
    }

    fn hydrate_documents(&self, result: QueryResult) -> Result<Vec<DynamicRelationRow>> {
        match result {
            QueryResult::Documents(docs) => coalesce_dynamic_relations(&self.descriptor, &docs),
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
    }
}

#[derive(Clone, Copy)]
enum ExactWritePolicy {
    Update,
    Put,
}

fn validate_relation_exact_write_input(
    descriptor: &RelationDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
    players: &[DynamicRolePlayerInput],
    policy: ExactWritePolicy,
    check_role_counts: bool,
) -> Result<()> {
    if matches!(policy, ExactWritePolicy::Update) {
        crate::_dynamic::validate_relation_iid(&descriptor.type_name, iid.unwrap_or_default())?;
    }
    validate_relation_exact_update_input_inner(
        descriptor,
        attributes,
        players,
        policy,
        check_role_counts,
    )
}

fn validate_relation_exact_update_input(
    descriptor: &RelationDescriptor,
    iid: &str,
    attributes: &DynamicAttributeMap,
    players: &[DynamicRolePlayerInput],
) -> Result<()> {
    validate_relation_exact_write_input(
        descriptor,
        Some(iid),
        attributes,
        players,
        ExactWritePolicy::Update,
        false,
    )
}

fn validate_relation_exact_put_input(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    players: &[DynamicRolePlayerInput],
) -> Result<()> {
    validate_relation_exact_write_input(
        descriptor,
        None,
        attributes,
        players,
        ExactWritePolicy::Put,
        false,
    )
}

fn validate_relation_exact_update_input_inner(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    players: &[DynamicRolePlayerInput],
    policy: ExactWritePolicy,
    check_role_counts: bool,
) -> Result<()> {
    if !type_bridge_core_lib::compiler::is_valid_typeql_label(&descriptor.type_name) {
        return Err(OrmError::QueryExecution(format!(
            "{}: unsafe relation type label",
            descriptor.type_name
        )));
    }
    let mut resolved_attributes = Vec::with_capacity(attributes.len());
    for (name, value) in attributes {
        let matches: Vec<_> = descriptor
            .owned_attributes
            .iter()
            .enumerate()
            .filter(|(_, a)| a.field_name == *name || a.attr_name == *name)
            .collect();
        if matches.is_empty() {
            return Err(OrmError::QueryExecution(format!(
                "{}: unknown relation attribute {name}",
                descriptor.type_name
            )));
        }
        if matches.len() != 1 {
            return Err(OrmError::QueryExecution(format!(
                "{}: ambiguous relation attribute {name}",
                descriptor.type_name
            )));
        }
        resolved_attributes.push((matches[0].0, value));
    }
    for (attr_index, attr) in descriptor.owned_attributes.iter().enumerate() {
        if !type_bridge_core_lib::compiler::is_valid_typeql_label(&attr.attr_name)
            || attr.attr_name.trim().is_empty()
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: unsafe relation attribute label {}",
                descriptor.type_name, attr.attr_name
            )));
        }
        let supplied = resolved_attributes
            .iter()
            .filter(|(candidate, _)| *candidate == attr_index)
            .count();
        let (min, max) = attr
            .cardinality()
            .unwrap_or((if attr.is_optional { 0 } else { 1 }, Some(1)));
        if (matches!(policy, ExactWritePolicy::Put) || !attr.is_key()) && supplied < min as usize {
            return Err(OrmError::QueryExecution(format!(
                "{}: relation attribute {} violates minimum cardinality",
                descriptor.type_name, attr.attr_name
            )));
        }
        if max.is_some_and(|m| supplied > m as usize) {
            return Err(OrmError::QueryExecution(format!(
                "{}: relation attribute {} violates maximum cardinality",
                descriptor.type_name, attr.attr_name
            )));
        }
        for (_, value) in resolved_attributes
            .iter()
            .filter(|(candidate, _)| *candidate == attr_index)
        {
            if value.value_type_name() != attr.value_type.as_str() {
                return Err(OrmError::QueryExecution(format!(
                    "{}: relation attribute {} has wrong value type",
                    descriptor.type_name, attr.attr_name
                )));
            }
        }
    }
    for role in &descriptor.roles {
        if !type_bridge_core_lib::compiler::is_valid_typeql_label(&role.role_name)
            || role.role_name.trim().is_empty()
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: unsafe relation role label {}",
                descriptor.type_name, role.role_name
            )));
        }
        if check_role_counts {
            let count = players
                .iter()
                .filter(|p| p.role_name == role.role_name)
                .count();
            let (min, max) = role.cardinality.unwrap_or((0, None));
            if count < min as usize || max.is_some_and(|m| count > m as usize) {
                return Err(OrmError::QueryExecution(format!(
                    "{}: relation role {} violates cardinality",
                    descriptor.type_name, role.role_name
                )));
            }
        }
    }
    for player in players {
        if descriptor
            .roles
            .iter()
            .all(|r| r.role_name != player.role_name)
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: unknown relation role {}",
                descriptor.type_name, player.role_name
            )));
        }
        if !type_bridge_core_lib::compiler::is_valid_typeql_label(&player.player_type_name) {
            return Err(OrmError::QueryExecution(format!(
                "{}: unsafe player type label {}",
                descriptor.type_name, player.player_type_name
            )));
        }
        if let Some((key, _)) = &player.key
            && (key.trim().is_empty()
                || !type_bridge_core_lib::compiler::is_valid_typeql_label(key))
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: unsafe player key label {}",
                descriptor.type_name, key
            )));
        }
        if player.iid.is_some() == player.key.is_some() {
            return Err(OrmError::QueryExecution(format!(
                "{}: player identity must be exactly IID xor key",
                descriptor.type_name
            )));
        }
        if let Some(iid) = &player.iid
            && !type_bridge_contract::id::is_canonical_thing_iid(iid)
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: player IID must be canonical",
                descriptor.type_name
            )));
        }
        if let Some((_, value)) = &player.key
            && crate::_dynamic::is_blank_key_value(value)
        {
            return Err(OrmError::QueryExecution(format!(
                "{}: player key value must be nonblank",
                descriptor.type_name
            )));
        }
    }
    if players.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "{}: exact relation {} requires at least one role player",
            descriptor.type_name,
            if matches!(policy, ExactWritePolicy::Put) {
                "put"
            } else {
                "update"
            }
        )));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum DynamicWriteOperation {
    Insert,
    Put,
}

async fn commit_manager(tx: &TransactionContext, plans: &Option<ManagerPlans>) -> Result<()> {
    if plans.is_some() {
        super::returning::commit(tx).await
    } else {
        tx.commit().await
    }
}

#[derive(Clone)]
enum DynamicExecutionTarget<'db> {
    Database(&'db Database),
    Transaction(TransactionContext),
}

impl DynamicExecutionTarget<'_> {
    fn answer_limits(&self) -> QueryExecutionResourceLimits {
        match self {
            Self::Database(db) => db.answer_limits(),
            Self::Transaction(tx) => tx.answer_limits(),
        }
        .unwrap_or_default()
    }

    async fn returning<T>(
        &self,
        plans: &ManagerPlans,
        name: &str,
        selection: super::returning::DeleteSelection<'_>,
        exact: bool,
        materialize: impl FnOnce(
            &DescriptorRegistry,
            &[(usize, &HydratedThing)],
            Vec<serde_json::Value>,
            QueryExecutionResourceLimits,
        ) -> Result<T>,
    ) -> Result<T> {
        plans.authority()?;
        let policy = match self {
            Self::Database(db) => db.answer_limits(),
            Self::Transaction(tx) => tx.answer_limits(),
        }
        .unwrap_or_default();
        let deadline = QueryExecutionDeadline::for_limits(policy);
        let cancellation = AnswerCancellation::default();
        let (transaction, owned) = match self {
            Self::Database(db) => (db.transaction_context(TxType::Write).await?, true),
            Self::Transaction(tx) => (tx.clone(), false),
        };
        let mut lease = transaction.acquire_mutation_lease().await?;
        let result = super::returning::delete_on_lease(
            &mut lease,
            plans,
            name,
            selection,
            exact,
            policy,
            &cancellation,
            deadline,
            materialize,
        )
        .await;
        match result {
            Ok(value) => {
                lease.complete_success();
                if owned {
                    super::returning::commit(&transaction).await?;
                }
                Ok(value)
            }
            Err(error) => {
                lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                    &error,
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                ));
                drop(lease);
                if owned {
                    let _ = transaction.rollback().await;
                }
                Err(error)
            }
        }
    }

    #[allow(
        clippy::manual_async_fn,
        reason = "avoid duplicate async argument captures in manager futures"
    )]
    fn read_plan(
        &self,
        program: &ManagerRead,
    ) -> impl std::future::Future<Output = Result<Vec<PlanAnswer>>> {
        async move {
            let limits = match program {
                ManagerRead::Fetch(stack) => {
                    stack.execution_limits(QueryExecutionResourceLimits::default())
                }
                ManagerRead::Scalar { .. } | ManagerRead::Aggregate { .. } => {
                    QueryExecutionResourceLimits::default()
                }
            };
            let cancellation = AnswerCancellation::default();
            let deadline = QueryExecutionDeadline::for_limits(limits);
            match self {
                Self::Database(db) => {
                    db.execute_plans(program.plans(), &limits, &cancellation, deadline)
                        .await
                }
                Self::Transaction(tx) => {
                    if tx.tx_type() == TxType::Read {
                        return tx
                            .execute_plans(program.plans(), &limits, &cancellation, deadline)
                            .await;
                    }
                    ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                    let limits = tx
                        .answer_limits()
                        .map_or(limits, |ceiling| limits.constrained_by(ceiling));
                    let mut lease = tx.acquire_mutation_lease().await?;
                    let result = lease
                        .execute_plans(program.plans(), limits, &cancellation, deadline, None)
                        .await;
                    match result {
                        Ok((answers, _)) => {
                            lease.complete_success();
                            Ok(answers)
                        }
                        Err(error) => {
                            lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                                &error,
                                type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                            ));
                            Err(error)
                        }
                    }
                }
            }
        }
    }

    async fn write_plan_group<T>(
        &self,
        program: crate::plan_builders::bulk_write::WriteProgram,
        map: impl FnOnce(Vec<PlanAnswer>) -> Result<T>,
    ) -> Result<T> {
        self.write_plan_group_mapped(program, |_, answer| Ok(answer), map)
            .await
    }

    async fn write_plan_group_mapped<T, U>(
        &self,
        mut program: crate::plan_builders::bulk_write::WriteProgram,
        mut map_step: impl FnMut(&BuiltPlan, PlanAnswer) -> Result<U>,
        map: impl FnOnce(Vec<U>) -> Result<T>,
    ) -> Result<T> {
        if let crate::plan_builders::bulk_write::WriteProgram::Compact(plans) = &program
            && let [plan] = plans.as_slice()
        {
            return self
                .write_plan(plan, |answer| {
                    map_step(plan, answer).and_then(|answer| map(vec![answer]))
                })
                .await;
        }
        program = program
            .prepare_static()
            .map_err(crate::plan_builders::manager::diagnostic)?;
        let statement_count = program.statement_count();
        let (tx, owned) = match self {
            Self::Database(db) => (db.transaction_context(TxType::Write).await?, true),
            Self::Transaction(tx) => (tx.clone(), false),
        };
        let policy = tx
            .answer_limits()
            .unwrap_or_default()
            .for_adapter_statements(statement_count);
        let cancellation = AnswerCancellation::default();
        let deadline = QueryExecutionDeadline::for_limits(policy);
        let given = tx.supports_given_stage().await?;
        let mut lease = tx.acquire_mutation_lease().await?;
        let result = async {
            if statement_count > policy.statement_limit() as usize {
                return Err(crate::match_request::MatchError::new(
                    crate::match_request::MatchErrorCategory::ResourceLimit,
                    "projected_batch_statement_limit",
                    "Manager batch exceeds the provider statement ceiling",
                )
                .into());
            }
            let mut remaining = policy;
            let mut answers = Vec::with_capacity(statement_count);
            while let Some(step) = program
                .next()
                .map_err(crate::plan_builders::manager::diagnostic)?
            {
                let records_answers = program.records_answers();
                let plan = &step.plan;
                lease.check_plan_provider(plan.local())?;
                let query = if given {
                    crate::query_plan_exec::staged_query(plan.local(), true)
                } else {
                    crate::query_plan_exec::staged_literal_query(plan.local())
                }
                .map_err(crate::query_plan_exec::plan_diagnostic_error)?;
                let request_bytes = (query.typeql.len() as u64).saturating_add(
                    query
                        .rows
                        .as_ref()
                        .map(crate::projected_batch_executor::given_rows_bytes)
                        .transpose()
                        .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?
                        .unwrap_or(0),
                );
                if request_bytes > remaining.bytes {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResourceLimit,
                        "projected_batch_transport_byte_limit",
                        "Manager batch exceeds the captured byte ceiling",
                    )
                    .into());
                }
                remaining.bytes -= request_bytes;
                let limits =
                    crate::query_plan_exec::plan_answer_limits(remaining, &cancellation, deadline)?;
                let (answer, stats) = lease
                    .dispatch_plan_counted(query, limits, Some(plan.local()))
                    .await?;
                let answer = TransactionContext::resolve_plan(plan.local(), answer)?;
                program
                    .record_answer(&step, &answer)
                    .map_err(crate::plan_builders::manager::diagnostic)?;
                if step.reference_resolution.is_some() {
                    let iids = step
                        .reference_iids(&answer)
                        .map_err(crate::plan_builders::manager::diagnostic)?;
                    if let Some(guard) = step
                        .reference_resolution
                        .as_ref()
                        .and_then(|resolution| resolution.guard.as_ref())
                    {
                        guard
                            .validate(&iids, |_, _| None)
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                    } else {
                        program
                            .accept_reference_iids(&step, iids)
                            .map_err(crate::plan_builders::manager::diagnostic)?;
                    }
                } else if step.candidate_filter {
                    program
                        .accept_candidates(&step, &answer)
                        .map_err(crate::plan_builders::manager::diagnostic)?;
                } else if step.complete {
                    answers.push(map_step(plan, answer)?);
                } else if !records_answers {
                    let iid = step
                        .identity(&answer)
                        .map_err(crate::plan_builders::manager::diagnostic)?;
                    program
                        .accept_identity(&step, iid)
                        .map_err(crate::plan_builders::manager::diagnostic)?;
                }
                remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
                remaining.items = remaining.items.saturating_sub(stats.processed_items);
            }
            let value = map(answers)?;
            deadline
                .check(&cancellation)
                .map_err(|diagnostic| OrmError::ExecutionDiagnostic(Box::new(diagnostic)))?;
            Ok(value)
        }
        .await;
        match result {
            Ok(value) => {
                lease.complete_success();
                if owned {
                    super::returning::commit(&tx).await?;
                }
                Ok(value)
            }
            Err(error) => {
                lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                    &error,
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                ));
                drop(lease);
                if owned {
                    let _ = tx.rollback().await;
                }
                Err(error)
            }
        }
    }

    async fn write_plan<T>(
        &self,
        plan: &BuiltPlan,
        map: impl FnOnce(PlanAnswer) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                match dispatch_manager_plan(&tx, plan, map).await {
                    Ok(answer) => {
                        super::returning::commit(&tx).await?;
                        Ok(answer)
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }
            Self::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                let result = dispatch_manager_plan(tx, plan, map).await;
                if let Err(error) = &result {
                    tx.latch_rollback_only(&crate::execution_diagnostic::lower_orm_error(
                        error,
                        type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                    ))
                    .await;
                }
                result
            }
        }
    }

    async fn delete_captured<F>(
        &self,
        statements: &(String, String),
        iids: &[String],
        hydrate: F,
    ) -> Result<Vec<serde_json::Value>>
    where
        F: FnMut(usize, Vec<serde_json::Value>) -> Result<Option<serde_json::Value>> + Send,
    {
        if iids.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen = std::collections::HashSet::new();
        for iid in iids {
            if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
                return Err(OrmError::QueryExecution(
                    "Delete requires a canonical IID".into(),
                ));
            }
            if !seen.insert(iid.to_ascii_lowercase()) {
                return Err(crate::match_request::MatchError::new(
                    crate::match_request::MatchErrorCategory::InvalidPlan,
                    "duplicate_batch_target",
                    "The same exact IID appears more than once in the batch",
                )
                .into());
            }
        }
        let canonical = false;
        match self {
            Self::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                match tx
                    .delete_captured(statements, iids, canonical, hydrate)
                    .await
                {
                    Ok(snapshots) => {
                        tx.commit_classified()
                            .await
                            .map_err(crate::error::ClassifiedCommitError::into_orm_error)?;
                        Ok(snapshots)
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        Err(error)
                    }
                }
            }
            Self::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                tx.delete_captured(statements, iids, canonical, hydrate)
                    .await
            }
        }
    }

    async fn execute(&self, typeql: &str, required_tx_type: TxType) -> Result<QueryResult> {
        // raw remainder: no registry authority; removed in 06
        match self {
            Self::Database(db) => db.execute_raw(typeql, required_tx_type).await,
            Self::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), required_tx_type)?;
                tx.query(typeql).await
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hydrate_write_rows<T>(
    plans: &ManagerPlans,
    plan: &BuiltPlan,
    name: &str,
    answer: PlanAnswer,
    limits: QueryExecutionResourceLimits,
    expected: Option<&str>,
    map: impl Fn(&DescriptorRegistry, &HydratedThing) -> Result<T>,
) -> Result<WriteResult<T>> {
    let PlanAnswer::Documents(documents) = answer else {
        return Err(snapshot_error(name));
    };
    let descriptor = plans
        .registry
        .descriptor_id(name)
        .ok_or_else(|| OrmError::DescriptorNotFound(name.into()))?;
    let (request, result) = crate::match_request::validated_match_result_from_plan(
        &plans.registry,
        plan.local(),
        &documents,
        &descriptor,
        limits,
    )?;
    let MatchResult::Rows { rows } = result.for_request(&request)? else {
        return Err(snapshot_error(name));
    };
    rows.iter()
        .map(|row| match row.slots() {
            [SlotValue::One(thing)] => {
                if expected
                    .is_some_and(|iid| !iid.eq_ignore_ascii_case(thing.concept_id().as_str()))
                {
                    return Err(snapshot_error(name));
                }
                map(&plans.registry, thing)
            }
            _ => Err(snapshot_error(name)),
        })
        .collect::<Result<Vec<_>>>()
        .map(WriteResult::new)
}

async fn dispatch_manager_plan<T>(
    tx: &TransactionContext,
    plan: &BuiltPlan,
    map: impl FnOnce(PlanAnswer) -> Result<T>,
) -> Result<T> {
    let policy = tx
        .answer_limits()
        .unwrap_or_default()
        .for_adapter_statements(1);
    if policy.statement_limit() < 1 {
        return Err(crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "statement_count_limit",
            "Manager write exceeds the provider statement ceiling",
        )
        .into());
    }
    let cancellation = AnswerCancellation::default();
    let deadline = QueryExecutionDeadline::for_limits(policy);
    let limits = crate::query_plan_exec::plan_answer_limits(policy, &cancellation, deadline)?;
    let mut lease = crate::session::staged::bounded(&limits.answer, async {
        tx.acquire_mutation_lease().await
    })
    .await?;
    let result = async {
        let query = lease.prepare_plan(plan.local())?;
        let request_bytes = (query.typeql.len() as u64).saturating_add(
            query
                .rows
                .as_ref()
                .map(crate::projected_batch_executor::given_rows_bytes)
                .transpose()
                .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?
                .unwrap_or(0),
        );
        if request_bytes > policy.bytes {
            return Err(crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::ResourceLimit,
                "projected_batch_transport_byte_limit",
                "Manager write exceeds the captured byte ceiling",
            )
            .into());
        }
        let remaining = QueryExecutionResourceLimits {
            bytes: policy.bytes - request_bytes,
            ..policy
        };
        let limits =
            crate::query_plan_exec::plan_answer_limits(remaining, &cancellation, deadline)?;
        let (answer, _) = lease
            .dispatch_plan_counted(query, limits, Some(plan.local()))
            .await?;
        let value = TransactionContext::resolve_plan(plan.local(), answer).and_then(map)?;
        deadline
            .check(&cancellation)
            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
        Ok(value)
    }
    .await;
    match result {
        Ok(value) => {
            lease.complete_success();
            Ok(value)
        }
        Err(error) => {
            lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                &error,
                type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
            ));
            Err(error)
        }
    }
}

fn hydrated_attributes(
    registry: &DescriptorRegistry,
    attributes: &[crate::match_request::HydratedAttribute],
) -> Result<DynamicAttributeMap> {
    let mut result = Vec::new();
    for attribute in attributes {
        let name = registry
            .provider_attribute_name(attribute.field())
            .ok_or_else(|| {
                crate::plan_builders::manager::invalid(
                    "manager_field_unknown",
                    "Hydrated manager field has no registered provider name",
                )
            })?;
        result.extend(
            attribute
                .values()
                .iter()
                .cloned()
                .map(|value| (name.clone(), value)),
        );
    }
    Ok(result)
}

pub(crate) fn plan_entity_row(
    registry: &DescriptorRegistry,
    thing: &HydratedThing,
) -> Result<DynamicEntityRow> {
    Ok(DynamicEntityRow {
        iid: Some(thing.concept_id().as_str().to_owned()),
        type_name: registry.descriptor_type_name(thing.concrete_descriptor()),
        attributes: hydrated_attributes(registry, thing.attributes())?,
    })
}

pub(crate) fn plan_relation_row(
    registry: &DescriptorRegistry,
    thing: &HydratedThing,
) -> Result<DynamicRelationRow> {
    let entity = plan_entity_row(registry, thing)?;
    let mut role_players = Vec::new();
    for role in thing.roles() {
        for player in role.players() {
            role_players.push(crate::_dynamic::DynamicRolePlayer {
                role_name: role.role().name.clone(),
                player_iid: Some(player.concept_id().as_str().into()),
                player_type_name: registry.descriptor_type_name(player.concrete_descriptor()),
                attributes: hydrated_attributes(registry, player.attributes())?
                    .into_iter()
                    .map(|(name, value)| (name, value.to_json()))
                    .collect(),
            });
        }
    }
    Ok(DynamicRelationRow {
        iid: entity.iid,
        type_name: entity.type_name,
        attributes: entity.attributes,
        role_players,
    })
}

#[allow(
    clippy::manual_async_fn,
    reason = "avoid duplicate async argument captures in manager futures"
)]
fn plan_fetch<T>(
    target: &DynamicExecutionTarget<'_>,
    plans: &ManagerPlans,
    program: ManagerRead,
    map: impl Fn(&DescriptorRegistry, &HydratedThing) -> Result<T>,
) -> impl std::future::Future<Output = Result<Vec<T>>> {
    async move {
        let answers = target.read_plan(&program).await?;
        let ManagerRead::Fetch(stack) = program else {
            unreachable!("model fetch program")
        };
        let limits = stack.execution_limits(QueryExecutionResourceLimits::default());
        let proof = stack.resolve(&plans.registry, &answers, limits)?;
        let MatchResult::Rows { rows } = proof.result().for_request(proof.request())? else {
            unreachable!("model fetch rows")
        };
        rows.iter()
            .map(|row| match row.slots() {
                [SlotValue::One(thing)] => map(&plans.registry, thing),
                _ => Err(crate::plan_builders::manager::invalid(
                    "manager_result_shape",
                    "A manager fetch requires one model slot",
                )),
            })
            .collect()
    }
}

fn equality_expressions(filters: &[Filter]) -> Vec<DynamicExpr> {
    filters
        .iter()
        .map(|filter| DynamicExpr::Compare {
            attr_name: filter.attr_name.clone(),
            operator: crate::_dynamic::DynamicComparisonOp::Eq,
            value: filter.value.clone(),
        })
        .collect()
}

#[allow(
    clippy::manual_async_fn,
    reason = "avoid duplicate async argument captures in manager futures"
)]
fn plan_read_count(
    target: &DynamicExecutionTarget<'_>,
    plans: &ManagerPlans,
    name: &str,
    exact: bool,
    expressions: &[DynamicExpr],
) -> impl std::future::Future<Output = Result<u64>> {
    async move {
        let program = plans.read(
            name,
            exact,
            expressions,
            &[],
            None,
            None,
            &[],
            ReadTerminal::Count,
        )?;
        let mut answers = target.read_plan(&program).await?;
        let answer = answers.pop().ok_or_else(|| snapshot_error(name))?;
        let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows { rows } = answer
            .to_staged_v3(&program.plans()[0])
            .map_err(crate::plan_builders::manager::diagnostic)?
        else {
            return Err(snapshot_error(name));
        };
        match rows.as_slice() {
            [row] => match row.as_slice() {
                [
                    type_bridge_contract::query_remote::RemoteValue::Value {
                        value: type_bridge_contract::value::CanonicalValue::Long(count),
                    },
                ] => u64::try_from(*count).map_err(|_| snapshot_error(name)),
                _ => Err(snapshot_error(name)),
            },
            _ => Err(snapshot_error(name)),
        }
    }
}

async fn plan_identities(
    target: &DynamicExecutionTarget<'_>,
    plans: &ManagerPlans,
    name: &str,
    exact: bool,
    iid: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let expressions = iid
        .map(|iid| DynamicExpr::Iid { iid: iid.into() })
        .into_iter()
        .collect::<Vec<_>>();
    let program = plans.read(
        name,
        exact,
        &expressions,
        &[],
        None,
        None,
        &[],
        ReadTerminal::Identity,
    )?;
    let rows = plan_identity_rows(target, &program).await?;
    let expected = plans
        .registry
        .descriptor_id(name)
        .ok_or_else(|| snapshot_error(name))?;
    if rows.iter().any(|(_, actual)| {
        plans
            .registry
            .descriptor_id(actual)
            .is_none_or(|actual| !plans.registry.is_same_or_subtype(&actual, &expected))
    }) {
        return Err(snapshot_error(name));
    }
    if iid.is_some_and(|expected| {
        rows.iter()
            .any(|(actual, _)| !expected.eq_ignore_ascii_case(actual))
    }) {
        return Err(snapshot_error(name));
    }
    Ok(rows)
}

async fn plan_identity_rows(
    target: &DynamicExecutionTarget<'_>,
    program: &ManagerRead,
) -> Result<Vec<(String, String)>> {
    let answers = target.read_plan(program).await?;
    let [answer] = answers.as_slice() else {
        return Err(snapshot_error("identity"));
    };
    let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows { rows } = answer
        .to_staged_v3(&program.plans()[0])
        .map_err(crate::plan_builders::manager::diagnostic)?
    else {
        return Err(snapshot_error("identity"));
    };
    rows.into_iter()
        .map(|mut row| match (row.len(), row.pop()) {
            (1, Some(type_bridge_contract::query_remote::RemoteValue::Thing { iid, type_id })) => {
                Ok((iid, type_id.label().as_str().into()))
            }
            _ => Err(snapshot_error("identity")),
        })
        .collect()
}

fn group_write_identity_rows(
    answers: Vec<PlanAnswer>,
    count: usize,
    layout: Option<crate::plan_builders::bulk_write::IdentityLayout>,
    name: &str,
) -> Result<Vec<String>> {
    use type_bridge_contract::query_remote::RemoteValue;
    use type_bridge_contract::value::CanonicalValue;
    let mut identities: Vec<Option<String>> = vec![None; count];
    if layout
        .as_ref()
        .is_some_and(|layout| layout.len() != answers.len())
    {
        return Err(snapshot_error(name));
    }
    for (piece, answer) in answers.into_iter().enumerate() {
        let PlanAnswer::Rows(rows) = answer else {
            return Err(snapshot_error(name));
        };
        for row in rows {
            let columns = if let Some(Some(columns)) = layout.as_ref().map(|layout| &layout[piece])
            {
                columns.clone()
            } else {
                let value = crate::query_plan_exec::row_value(&row, "ordinal1", Default::default())
                    .map_err(crate::plan_builders::manager::diagnostic)?;
                let RemoteValue::Value {
                    value: CanonicalValue::Long(ordinal),
                } = value
                else {
                    return Err(snapshot_error(name));
                };
                vec![(
                    usize::try_from(ordinal).map_err(|_| snapshot_error(name))?,
                    String::from("root0"),
                )]
            };
            for (ordinal, column) in columns {
                let value = crate::query_plan_exec::row_value(&row, &column, Default::default())
                    .map_err(crate::plan_builders::manager::diagnostic)?;
                let RemoteValue::Thing { iid, .. } = value else {
                    return Err(snapshot_error(name));
                };
                let target = identities
                    .get_mut(ordinal)
                    .ok_or_else(|| snapshot_error(name))?;
                if target.as_ref().is_some_and(|previous| previous != &iid) {
                    return Err(snapshot_error(name));
                }
                *target = Some(iid);
            }
        }
    }
    identities
        .into_iter()
        .map(|iid| iid.ok_or_else(|| snapshot_error(name)))
        .collect()
}

fn group_write_documents(
    answers: Vec<PlanAnswer>,
    count: usize,
    name: &str,
) -> Result<Vec<Vec<serde_json::Value>>> {
    let mut groups = vec![Vec::new(); count];
    for answer in answers {
        let PlanAnswer::Documents(documents) = answer else {
            return Err(snapshot_error(name));
        };
        for document in documents {
            let ordinal = document
                .get("ordinal")
                .and_then(serde_json::Value::as_u64)
                .and_then(|ordinal| usize::try_from(ordinal).ok())
                .ok_or_else(|| snapshot_error(name))?;
            groups
                .get_mut(ordinal)
                .ok_or_else(|| snapshot_error(name))?
                .push(document);
        }
    }
    if groups.iter().any(Vec::is_empty) {
        return Err(snapshot_error(name));
    }
    Ok(groups)
}

fn validate_written_players(
    registry: &DescriptorRegistry,
    exact: bool,
    name: &str,
    row: &DynamicRelationRow,
    players: &[DynamicRolePlayerInput],
) -> Result<()> {
    let mut expected = std::collections::BTreeMap::<_, usize>::new();
    let mut actual = std::collections::BTreeMap::<_, usize>::new();
    for player in players {
        *expected.entry(player.role_name.as_str()).or_default() += 1;
    }
    for player in &row.role_players {
        *actual.entry(player.role_name.as_str()).or_default() += 1;
    }
    if expected != actual {
        return Err(snapshot_error(name));
    }
    for expected in players {
        let found = row.role_players.iter().any(|actual| {
            actual.role_name == expected.role_name
                && expected.iid.as_deref().is_none_or(|iid| {
                    actual
                        .player_iid
                        .as_deref()
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(iid))
                })
                && expected.key.as_ref().is_none_or(|(field, value)| {
                    actual.attributes.iter().any(|(name, actual)| {
                        name == field
                            && crate::value::AttributeValue::from_json(
                                actual,
                                value.value_type_name(),
                            )
                            .as_ref()
                            .is_some_and(|actual| {
                                crate::runtime_projection::canonical_attribute_value(actual).ok()
                                    == crate::runtime_projection::canonical_attribute_value(value)
                                        .ok()
                            })
                    })
                })
                && actual.player_type_name.as_deref().is_some_and(|actual| {
                    if actual == expected.player_type_name {
                        return true;
                    }
                    if exact {
                        return false;
                    }
                    registry
                        .installed_schema_authority()
                        .expect("registry-backed write")
                        .resolved()
                        .types()
                        .iter()
                        .any(|(id, model)| {
                            id.label().as_str() == expected.player_type_name
                                && model
                                    .subtypes()
                                    .iter()
                                    .any(|child| child.label().as_str() == actual)
                        })
                })
        });
        if !found {
            return Err(snapshot_error(name));
        }
    }
    Ok(())
}

async fn plan_aggregate(
    target: &DynamicExecutionTarget<'_>,
    program: ManagerRead,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let answers = target.read_plan(&program).await?;
    let ManagerRead::Aggregate { plan, keys } = program else {
        unreachable!("aggregate program")
    };
    let [answer] = answers.as_slice() else {
        return Err(snapshot_error("aggregate"));
    };
    let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows { rows } = answer
        .to_staged_v3(plan.local())
        .map_err(crate::plan_builders::manager::diagnostic)?
    else {
        return Err(snapshot_error("aggregate"));
    };
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .zip(&keys)
                .map(|(value, key)| {
                    let value = match value {
                        type_bridge_contract::query_remote::RemoteValue::Value { value }
                        | type_bridge_contract::query_remote::RemoteValue::Attribute {
                            value,
                            ..
                        } => crate::runtime_projection::attribute_value_from_canonical(&value)
                            .to_json(),
                        type_bridge_contract::query_remote::RemoteValue::Absent => {
                            serde_json::Value::Null
                        }
                        _ => return Err(snapshot_error("aggregate")),
                    };
                    Ok((key.clone(), value))
                })
                .collect()
        })
        .collect()
}

fn relation_snapshot_document(
    descriptor: &RelationDescriptor,
    iid: &str,
    documents: Vec<serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let rows = coalesce_dynamic_relation_by_iid(descriptor, &documents, &iid.to_ascii_lowercase())?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    validate_snapshot_identity(
        &descriptor.type_name,
        iid,
        row.iid.as_deref(),
        row.type_name.as_deref(),
    )?;
    let mut document = documents
        .into_iter()
        .next()
        .ok_or_else(|| snapshot_error(&descriptor.type_name))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| snapshot_error(&descriptor.type_name))?;
    object.retain(|key, _| !key.starts_with("_role_"));
    let players = row
        .role_players
        .into_iter()
        .map(|player| {
            let attributes: serde_json::Map<String, serde_json::Value> =
                player.attributes.into_iter().collect();
            serde_json::json!({"role": player.role_name, "iid": player.player_iid,
            "type_name": player.player_type_name, "attributes": attributes})
        })
        .collect();
    object.insert("role_players".into(), serde_json::Value::Array(players));
    Ok(Some(document))
}

fn snapshot_documents(
    things: &[(usize, &HydratedThing)],
    documents: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    let mut by_iid = std::collections::HashMap::new();
    for document in documents {
        let iid = document
            .get("_iid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| snapshot_error("snapshot"))?
            .to_owned();
        by_iid.entry(iid).or_insert(document);
    }
    things
        .iter()
        .map(|(_, thing)| {
            by_iid
                .remove(thing.concept_id().as_str())
                .ok_or_else(|| snapshot_error("snapshot"))
        })
        .collect()
}

fn snapshot_error(type_name: &str) -> OrmError {
    OrmError::Hydration {
        type_name: type_name.into(),
        message: "Manager answer has invalid identity evidence".into(),
    }
}

pub(super) fn validate_delete_iids(iids: &[String]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for iid in iids {
        if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
            return Err(crate::plan_builders::manager::invalid(
                "query_plan_invalid_iid",
                "Deletion requires canonical IIDs",
            ));
        }
        if !seen.insert(iid.to_ascii_lowercase()) {
            return Err(crate::plan_builders::manager::invalid(
                "duplicate_batch_target",
                "The same IID appears more than once in a deletion batch",
            ));
        }
    }
    Ok(())
}

fn validate_snapshot_identity(
    type_name: &str,
    expected_iid: &str,
    iid: Option<&str>,
    concrete_type: Option<&str>,
) -> Result<()> {
    if iid.is_none_or(|iid| !iid.eq_ignore_ascii_case(expected_iid))
        || concrete_type != Some(type_name)
    {
        return Err(snapshot_error(type_name));
    }
    Ok(())
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

fn extract_exists(type_name: &str, result: QueryResult) -> Result<bool> {
    match result {
        QueryResult::Documents(documents) => Ok(!documents.is_empty()),
        QueryResult::Ok => Err(OrmError::Hydration {
            type_name: type_name.to_owned(),
            message: "Expected Documents from limit-one existence query, got Ok".into(),
        }),
        QueryResult::Rows(_) => Err(OrmError::Hydration {
            type_name: type_name.to_owned(),
            message: "Expected Documents from limit-one existence query, got Rows".into(),
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
mod preflight_tests;
