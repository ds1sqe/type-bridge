//! Dynamic managers backed by runtime descriptors.

use std::sync::Arc;

use crate::descriptor::{EntityDescriptor, RelationDescriptor};
use crate::dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayerInput,
};
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

use super::hydration::{extract_count, hydrate_dynamic_entity, hydrate_dynamic_relation};
use super::query_builder;

/// CRUD manager for an entity described at runtime.
pub struct DynamicEntityManager<'db> {
    db: &'db Database,
    descriptor: Arc<EntityDescriptor>,
}

impl<'db> DynamicEntityManager<'db> {
    /// Create a dynamic entity manager from a registered descriptor.
    pub fn new(db: &'db Database, descriptor: Arc<EntityDescriptor>) -> Self {
        Self { db, descriptor }
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
        let result = self.db.execute_raw(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Fetch entities matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicEntityRow>> {
        let typeql = query_builder::build_dynamic_entity_fetch(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC FETCH");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
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
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Delete one entity by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let typeql =
            query_builder::build_dynamic_entity_delete_by_iid(&self.descriptor, iid, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC DELETE");
        self.db.execute_raw(&typeql, TxType::Write).await?;
        Ok(())
    }
}

/// CRUD manager for a relation described at runtime.
pub struct DynamicRelationManager<'db> {
    db: &'db Database,
    descriptor: Arc<RelationDescriptor>,
}

impl<'db> DynamicRelationManager<'db> {
    /// Create a dynamic relation manager from a registered descriptor.
    pub fn new(db: &'db Database, descriptor: Arc<RelationDescriptor>) -> Self {
        Self { db, descriptor }
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
        let result = self.db.execute_raw(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Fetch relations matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicRelationRow>> {
        let typeql = query_builder::build_dynamic_relation_fetch(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
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
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Delete one relation by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let typeql =
            query_builder::build_dynamic_relation_delete_by_iid(&self.descriptor, iid, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION DELETE");
        self.db.execute_raw(&typeql, TxType::Write).await?;
        Ok(())
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
