//! Read-only schema introspection.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::session::Database;
use crate::session::backend::TxType;
use type_bridge_core_lib::__internal as tql;

use super::info::*;

/// Reads a live database schema as [`SchemaInfo`].
///
/// Schema changes go through Split-YAML and reviewed migrations; this type
/// never defines schema.
pub struct SchemaManager<'db> {
    db: &'db Database,
}

impl<'db> SchemaManager<'db> {
    /// Create a schema reader for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Read the existing schema from the database and return it as [`SchemaInfo`].
    ///
    /// Queries TypeDB's type system for all entity types, relation types,
    /// attribute types, ownership, and roles, then assembles them into a
    /// [`SchemaInfo`].
    ///
    /// # Example
    ///
    /// This example is ignored because schema introspection requires a live
    /// TypeDB service.
    ///
    /// ```ignore
    /// let live = SchemaManager::new(&db).introspect().await?;
    /// println!("{} entity types", live.entities.len());
    /// ```
    #[tracing::instrument(skip(self))]
    pub async fn introspect(&self) -> Result<SchemaInfo> {
        use crate::session::backend::QueryResult;

        if let Ok(typeql) = self.db.schema_text().await {
            return Ok(SchemaInfo::from_typeql(&typeql)?);
        }

        let mut info = SchemaInfo::default();

        // ── 1. Query all user-defined attribute types ──────────────
        // Schema-only read, excepted like define/undefine (06.8).
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
        // Schema-only read, excepted like define/undefine (06.8).
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
        // Schema-only read, excepted like define/undefine (06.8).
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

        // Schema-only read, excepted like define/undefine (06.8).
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

        // Schema-only read, excepted like define/undefine (06.8).
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
