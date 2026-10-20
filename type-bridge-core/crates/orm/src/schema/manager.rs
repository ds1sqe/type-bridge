//! Schema manager for registering models and syncing schema.

use std::collections::BTreeMap;

use crate::_entity::TypeBridgeEntity;
use crate::_relation::TypeBridgeRelation;
use crate::error::Result;
use crate::session::backend::TxType;
use crate::session::{Database, require_legacy_writer_open_in_transaction};
use type_bridge_core_lib::__internal as tql;

use super::error::SchemaError;
use super::info::*;

/// Convert a static `(&str, &str)` meta slice into the owned map the schema IR uses.
fn meta_map(pairs: &[(&'static str, &'static str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Manages schema registration, generation, and synchronization.
///
/// # Example
///
/// This example is ignored because it requires a live TypeDB service and
/// generated schema types supplied by the consuming package.
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
                self.info
                    .attributes
                    .entry(a.attr_name.to_string())
                    .or_insert_with(|| AttributeSchemaEntry::new(a.attr_name, a.value_type));
                OwnedAttributeEntry {
                    attr_name: a.attr_name.to_string(),
                    value_type: a.value_type,
                    annotations: a.annotations.to_vec(),
                    is_ordered: false,
                    doc: a.doc.map(String::from),
                    meta: meta_map(a.meta),
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
                plays_cardinalities: BTreeMap::new(),
                doc: E::DOC.map(String::from),
                meta: meta_map(E::META),
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
                self.info
                    .attributes
                    .entry(a.attr_name.to_string())
                    .or_insert_with(|| AttributeSchemaEntry::new(a.attr_name, a.value_type));
                OwnedAttributeEntry {
                    attr_name: a.attr_name.to_string(),
                    value_type: a.value_type,
                    annotations: a.annotations.to_vec(),
                    is_ordered: false,
                    doc: a.doc.map(String::from),
                    meta: meta_map(a.meta),
                }
            })
            .collect();

        let roles: Vec<RoleEntry> = R::role_info()
            .iter()
            .map(|r| RoleEntry {
                role_name: r.role_name.to_string(),
                player_type_names: vec![r.player_type_name.to_string()],
                doc: r.doc.map(String::from),
                meta: meta_map(r.meta),
                ..Default::default()
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
                plays_cardinalities: BTreeMap::new(),
                doc: R::DOC.map(String::from),
                meta: meta_map(R::META),
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
            let typeql = existence_query(entity_name).map_err(query_error)?;
            match self.db.execute_raw(&typeql, TxType::Read).await {
                Ok(_) => return Ok(true),
                Err(_) => return Ok(false),
            }
        }
        // Try matching the first relation type
        if let Some(relation_name) = self.info.relations.keys().next() {
            let typeql = existence_query(relation_name).map_err(query_error)?;
            match self.db.execute_raw(&typeql, TxType::Read).await {
                Ok(_) => return Ok(true),
                Err(_) => return Ok(false),
            }
        }
        Ok(false)
    }

    /// Read the existing schema from the database and return it as [`SchemaInfo`].
    ///
    /// Queries TypeDB's type system for all entity types, relation types,
    /// attribute types, ownership, and roles, then assembles them into a
    /// [`SchemaInfo`] that can be compared against the registered (desired) schema.
    ///
    /// # Example
    ///
    /// This example is ignored because schema introspection requires a live
    /// TypeDB service and generated desired-schema registrations.
    ///
    /// ```ignore
    /// let schema = SchemaManager::new(&db);
    /// let live = schema.introspect().await?;
    /// let diff = schema.schema_info().compare(&live);
    /// println!("{}", diff.summary());
    /// ```
    #[tracing::instrument(skip(self))]
    pub async fn introspect(&self) -> Result<SchemaInfo> {
        use crate::session::backend::QueryResult;

        if let Ok(typeql) = self.db.schema_text().await {
            return Ok(SchemaInfo::from_typeql(&typeql)?);
        }

        let mut info = SchemaInfo::default();

        // ── 1. Query all user-defined attribute types ──────────────
        let attr_query = schema_scan_query("attribute").map_err(query_error)?;
        if let Ok(QueryResult::Documents(docs)) =
            self.db.execute_raw(&attr_query, TxType::Read).await
        {
            for doc in &docs {
                if let (Some(name), Some(vt_str)) = (
                    doc.get("name")
                        .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str())),
                    doc.get("value_type")
                        .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str())),
                ) && let Some(vt) = parse_value_type(vt_str)
                {
                    info.attributes
                        .insert(name.to_string(), AttributeSchemaEntry::new(name, vt));
                }
            }
        }

        // ── 2. Query all entity types with owned attributes ────────
        let entity_query = schema_scan_query("entity").map_err(query_error)?;
        if let Ok(QueryResult::Documents(docs)) =
            self.db.execute_raw(&entity_query, TxType::Read).await
        {
            for doc in &docs {
                if let Some(name) = doc
                    .get("name")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                {
                    let owned = self
                        .introspect_owned_attributes(name, &info.attributes)
                        .await;
                    info.entities.insert(
                        name.to_string(),
                        EntitySchemaEntry {
                            type_name: name.to_string(),
                            is_abstract: false,
                            parent_type: None,
                            owned_attributes: owned,
                            plays_cardinalities: BTreeMap::new(),
                            doc: None,
                            meta: Default::default(),
                        },
                    );
                }
            }
        }

        // ── 3. Query all relation types with roles and attributes ──
        let relation_query = schema_scan_query("relation").map_err(query_error)?;
        if let Ok(QueryResult::Documents(docs)) =
            self.db.execute_raw(&relation_query, TxType::Read).await
        {
            for doc in &docs {
                if let Some(name) = doc
                    .get("name")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                {
                    let owned = self
                        .introspect_owned_attributes(name, &info.attributes)
                        .await;
                    let roles = self.introspect_roles(name).await;
                    info.relations.insert(
                        name.to_string(),
                        RelationSchemaEntry {
                            type_name: name.to_string(),
                            is_abstract: false,
                            parent_type: None,
                            owned_attributes: owned,
                            roles,
                            plays_cardinalities: BTreeMap::new(),
                            doc: None,
                            meta: Default::default(),
                        },
                    );
                }
            }
        }

        Ok(info)
    }

    /// Query the owned attributes of a specific type.
    async fn introspect_owned_attributes(
        &self,
        type_name: &str,
        known_attrs: &std::collections::BTreeMap<String, AttributeSchemaEntry>,
    ) -> Vec<OwnedAttributeEntry> {
        use crate::session::backend::QueryResult;

        let Ok(query) = schema_capability_query(type_name, false) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        if let Ok(QueryResult::Documents(docs)) = self.db.execute_raw(&query, TxType::Read).await {
            for doc in &docs {
                if let Some(attr_name) = doc
                    .get("attr")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                {
                    let value_type = known_attrs
                        .get(attr_name)
                        .map(|a| a.value_type)
                        .unwrap_or(crate::_attribute::ValueType::String);
                    entries.push(OwnedAttributeEntry {
                        attr_name: attr_name.to_string(),
                        value_type,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    });
                }
            }
        }
        entries
    }

    /// Query the roles of a relation type.
    async fn introspect_roles(&self, relation_name: &str) -> Vec<RoleEntry> {
        use crate::session::backend::QueryResult;

        let Ok(query) = schema_capability_query(relation_name, true) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        if let Ok(QueryResult::Documents(docs)) = self.db.execute_raw(&query, TxType::Read).await {
            for doc in &docs {
                if let Some(role_name) = doc
                    .get("role")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                {
                    entries.push(RoleEntry {
                        // TypeDB returns scoped role labels; the schema IR stores
                        // the local name used by a relates declaration.
                        role_name: role_name
                            .rsplit_once(':')
                            .map_or(role_name, |(_, name)| name)
                            .to_owned(),
                        ..Default::default()
                    });
                }
            }
        }
        entries
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

        let typeql = self
            .generate_schema()
            .map_err(crate::error::OrmError::Schema)?;
        self.db.check_schema_annotation_support(&typeql)?;
        tracing::debug!(typeql = %typeql, "Syncing schema to database");
        let transaction = self.db.transaction_context(TxType::Schema).await?;
        if let Err(error) = require_legacy_writer_open_in_transaction(&transaction).await {
            let _ = transaction.close().await;
            return Err(error);
        }
        transaction.query(&typeql).await?;
        transaction.commit().await?;
        Ok(())
    }
}

/// Parse a value type string from TypeDB introspection into our enum.
fn parse_value_type(s: &str) -> Option<crate::_attribute::ValueType> {
    use crate::_attribute::ValueType;
    match s {
        "string" => Some(ValueType::String),
        "long" => Some(ValueType::Long),
        "double" => Some(ValueType::Double),
        "boolean" => Some(ValueType::Boolean),
        "date" => Some(ValueType::Date),
        "datetime" => Some(ValueType::DateTime),
        "datetime-tz" => Some(ValueType::DateTimeTz),
        "decimal" => Some(ValueType::Decimal),
        "duration" => Some(ValueType::Duration),
        _ => None,
    }
}

fn existence_query(name: &str) -> std::result::Result<String, tql::TqlError> {
    tql::render(&tql::pipeline(vec![
        tql::match_stage(vec![tql::thing("x", vec![tql::isa(name, false)?])?]),
        tql::limit(1),
    ]))
}
fn schema_scan_query(kind: &str) -> std::result::Result<String, tql::TqlError> {
    let mut fetch = vec![(
        "name",
        tql::fetch_expr(tql::function("label", vec![tql::var("t")?])?),
    )];
    if kind == "attribute" {
        fetch.push((
            "value_type",
            tql::fetch_expr(tql::function("value_type", vec![tql::var("t")?])?),
        ));
    }
    tql::render(&tql::pipeline(vec![
        tql::match_stage(vec![tql::schema_type_pattern("$t", "sub!", kind)?]),
        tql::fetch(fetch)?,
    ]))
}
fn schema_capability_query(name: &str, role: bool) -> std::result::Result<String, tql::TqlError> {
    let (kind, variable, key) = if role {
        ("relates", "$r", "role")
    } else {
        ("owns", "$a", "attr")
    };
    tql::render(&tql::pipeline(vec![
        tql::match_stage(vec![
            tql::schema_type_pattern("$t", "label", name)?,
            tql::schema_type_pattern("$t", kind, variable)?,
        ]),
        tql::fetch(vec![(
            key,
            tql::fetch_expr(tql::function("label", vec![tql::var(variable)?])?),
        )])?,
    ]))
}
fn query_error(error: tql::TqlError) -> crate::error::OrmError {
    crate::error::OrmError::Compilation(error.to_string())
}

#[cfg(test)]
mod ast_query_tests {
    use super::*;
    fn canonical(source: &str) -> typeql::Query {
        typeql::parse_query(&typeql::parse_query(source).unwrap().to_string()).unwrap()
    }
    #[test]
    fn introspection_queries_preserve_valid_ast_semantics() {
        for (new, old) in [
            (
                existence_query("person").unwrap(),
                "match $x isa person; limit 1;",
            ),
            (
                schema_scan_query("attribute").unwrap(),
                r#"match $t sub! attribute; fetch {"name":label($t),"value_type":value_type($t)};"#,
            ),
            (
                schema_scan_query("entity").unwrap(),
                r#"match $t sub! entity; fetch {"name":label($t)};"#,
            ),
            (
                schema_scan_query("relation").unwrap(),
                r#"match $t sub! relation; fetch {"name":label($t)};"#,
            ),
        ] {
            assert_eq!(canonical(&new), canonical(old));
        }
    }
    #[test]
    fn corrected_type_label_introspection_parses_and_rejects_injection() {
        for role in [false, true] {
            canonical(&schema_capability_query("person", role).unwrap());
            assert!(schema_capability_query("person; entity injected", role).is_err());
        }
        assert!(typeql::parse_query("match $t type person; $t owns $a;").is_err());
    }
}
