//! Schema manager for registering models and syncing schema.

use crate::entity::TypeBridgeEntity;
use crate::error::Result;
use crate::relation::TypeBridgeRelation;
use crate::session::backend::TxType;
use crate::session::Database;

use super::error::SchemaError;
use super::info::*;

/// Manages schema registration, generation, and synchronization.
///
/// # Example
///
/// ```ignore
/// let mut schema = SchemaManager::new(&db);
/// schema.register_entity::<Person>();
/// schema.register_entity::<Company>();
/// schema.register_relation::<Employment>();
///
/// let typeql = schema.generate_schema()?;
/// schema.sync_schema(false, false).await?;
/// ```
pub struct SchemaManager<'db> {
    db: &'db Database,
    info: SchemaInfo,
}

impl<'db> SchemaManager<'db> {
    /// Create a new schema manager for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self {
            db,
            info: SchemaInfo::default(),
        }
    }

    /// Register an entity type, extracting its metadata.
    #[tracing::instrument(skip(self), fields(entity_type = E::TYPE_NAME))]
    pub fn register_entity<E: TypeBridgeEntity>(&mut self) {
        let owned = E::owned_attributes();
        let owned_entries: Vec<OwnedAttributeEntry> = owned
            .iter()
            .map(|a| {
                // Also register the attribute type
                self.info.attributes.entry(a.attr_name.to_string()).or_insert(
                    AttributeSchemaEntry {
                        attr_name: a.attr_name.to_string(),
                        value_type: a.value_type,
                    },
                );
                OwnedAttributeEntry {
                    attr_name: a.attr_name.to_string(),
                    value_type: a.value_type,
                    annotations: a.annotations.to_vec(),
                }
            })
            .collect();

        self.info.entities.insert(
            E::TYPE_NAME.to_string(),
            EntitySchemaEntry {
                type_name: E::TYPE_NAME.to_string(),
                is_abstract: E::IS_ABSTRACT,
                parent_type: E::PARENT_TYPE.map(String::from),
                owned_attributes: owned_entries,
            },
        );
    }

    /// Register a relation type, extracting its metadata and roles.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub fn register_relation<R: TypeBridgeRelation>(&mut self) {
        let owned = R::owned_attributes();
        let owned_entries: Vec<OwnedAttributeEntry> = owned
            .iter()
            .map(|a| {
                self.info.attributes.entry(a.attr_name.to_string()).or_insert(
                    AttributeSchemaEntry {
                        attr_name: a.attr_name.to_string(),
                        value_type: a.value_type,
                    },
                );
                OwnedAttributeEntry {
                    attr_name: a.attr_name.to_string(),
                    value_type: a.value_type,
                    annotations: a.annotations.to_vec(),
                }
            })
            .collect();

        let roles: Vec<RoleEntry> = R::role_info()
            .iter()
            .map(|r| RoleEntry {
                role_name: r.role_name.to_string(),
                player_type_name: r.player_type_name.to_string(),
            })
            .collect();

        self.info.relations.insert(
            R::TYPE_NAME.to_string(),
            RelationSchemaEntry {
                type_name: R::TYPE_NAME.to_string(),
                is_abstract: R::IS_ABSTRACT,
                parent_type: R::PARENT_TYPE.map(String::from),
                owned_attributes: owned_entries,
                roles,
            },
        );
    }

    /// Get a reference to the collected schema info.
    pub fn schema_info(&self) -> &SchemaInfo {
        &self.info
    }

    /// Validate and generate a TypeQL `define` block.
    #[tracing::instrument(skip(self))]
    pub fn generate_schema(&self) -> std::result::Result<String, SchemaError> {
        self.info.to_typeql()
    }

    /// Best-effort check whether any registered types already exist in the database.
    ///
    /// Tries a simple match query for the first registered entity or relation type.
    #[tracing::instrument(skip(self))]
    pub async fn has_existing_schema(&self) -> Result<bool> {
        // Try matching the first entity type
        if let Some(entity_name) = self.info.entities.keys().next() {
            let typeql = format!("match $x isa {entity_name}; limit 1;");
            match self.db.execute_raw(&typeql, TxType::Read).await {
                Ok(_) => return Ok(true),
                Err(_) => return Ok(false),
            }
        }
        // Try matching the first relation type
        if let Some(relation_name) = self.info.relations.keys().next() {
            let typeql = format!("match $x isa {relation_name}; limit 1;");
            match self.db.execute_raw(&typeql, TxType::Read).await {
                Ok(_) => return Ok(true),
                Err(_) => return Ok(false),
            }
        }
        Ok(false)
    }

    /// Synchronize the schema to the database.
    ///
    /// - `force`: Skip existence check, always execute.
    /// - `skip_if_exists`: If types already exist, return Ok silently.
    #[tracing::instrument(skip(self), fields(force, skip_if_exists))]
    pub async fn sync_schema(
        &self,
        force: bool,
        skip_if_exists: bool,
    ) -> std::result::Result<(), crate::error::OrmError> {
        if !force {
            match self.has_existing_schema().await {
                Ok(true) => {
                    if skip_if_exists {
                        return Ok(());
                    }
                    return Err(crate::error::OrmError::Schema(SchemaError::Sync(
                        "Schema types already exist. Use force=true to overwrite.".into(),
                    )));
                }
                Ok(false) => {}
                Err(e) => {
                    // Existence check failed — probably no schema exists
                    tracing::debug!("Schema existence check failed (probably no schema): {e}");
                }
            }
        }

        let typeql = self.generate_schema().map_err(crate::error::OrmError::Schema)?;
        tracing::debug!(typeql = %typeql, "Syncing schema to database");
        self.db.execute_raw(&typeql, TxType::Schema).await?;
        Ok(())
    }
}
