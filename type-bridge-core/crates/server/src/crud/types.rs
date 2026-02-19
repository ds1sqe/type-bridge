//! Request and response types for CRUD endpoints.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Request body for inserting a new entity.
///
/// # Example
///
/// ```json
/// {
///     "database": "my_db",
///     "attributes": {
///         "name": { "value": "Alice", "value_type": "string" },
///         "age": { "value": 30, "value_type": "long" }
///     }
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct EntityInsertRequest {
    /// Optional database override (uses pipeline default if not specified).
    pub database: Option<String>,
    /// Attribute name-to-value map.
    pub attributes: HashMap<String, AttributeValueSpec>,
}

/// Request body for fetching entities with optional filters.
///
/// Used as query parameters on GET requests.
#[derive(Debug, Deserialize)]
pub struct EntityFetchRequest {
    /// Optional database override.
    pub database: Option<String>,
    /// Optional filter specifications.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// Optional sort specifications.
    #[serde(default)]
    pub sort: Vec<SortSpec>,
    /// Maximum number of results.
    pub limit: Option<u64>,
    /// Number of results to skip.
    pub offset: Option<u64>,
}

/// Request body for updating an entity's attributes.
#[derive(Debug, Deserialize)]
pub struct EntityUpdateRequest {
    /// Optional database override.
    pub database: Option<String>,
    /// New attribute values to set.
    pub attributes: HashMap<String, AttributeValueSpec>,
}

/// Request body for inserting a new relation.
///
/// # Example
///
/// ```json
/// {
///     "database": "my_db",
///     "role_players": [
///         { "role": "employee", "entity_type": "person", "key_attr": "name", "key_value": { "value": "Alice", "value_type": "string" } },
///         { "role": "employer", "entity_type": "company", "key_attr": "name", "key_value": { "value": "Acme", "value_type": "string" } }
///     ],
///     "attributes": {
///         "start-date": { "value": "2024-01-01", "value_type": "date" }
///     }
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct RelationInsertRequest {
    /// Optional database override.
    pub database: Option<String>,
    /// Role player specifications for the relation.
    pub role_players: Vec<RolePlayerSpec>,
    /// Optional attributes on the relation itself.
    #[serde(default)]
    pub attributes: HashMap<String, AttributeValueSpec>,
}

/// A typed attribute value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeValueSpec {
    /// The raw JSON value (string, number, boolean, etc.).
    pub value: serde_json::Value,
    /// The TypeDB value type (e.g. "string", "long", "double", "boolean", "datetime").
    pub value_type: String,
}

/// A filter specification for query endpoints.
#[derive(Debug, Deserialize)]
pub struct FilterSpec {
    /// The attribute name to filter on.
    pub attr: String,
    /// The comparison operator (e.g. "==", "!=", ">", "<", ">=", "<=", "contains", "like").
    pub op: String,
    /// The value to compare against.
    pub value: AttributeValueSpec,
}

/// A sort specification for query endpoints.
#[derive(Debug, Deserialize)]
pub struct SortSpec {
    /// The attribute name to sort by.
    pub attr: String,
    /// Sort direction: "asc" or "desc".
    #[serde(default = "default_sort_dir")]
    pub dir: String,
}

fn default_sort_dir() -> String {
    "asc".to_string()
}

/// Specification for a role player in a relation insert.
#[derive(Debug, Deserialize)]
pub struct RolePlayerSpec {
    /// The role name (e.g. "employee", "employer").
    pub role: String,
    /// The entity type of the role player.
    pub entity_type: String,
    /// Optional IID to identify the role player directly.
    pub iid: Option<String>,
    /// Optional key attribute name to find the role player by.
    pub key_attr: Option<String>,
    /// Optional key attribute value to find the role player by.
    pub key_value: Option<AttributeValueSpec>,
}

/// Unified CRUD response.
#[derive(Debug, Serialize)]
pub struct CrudResponse {
    /// Status indicator ("ok" on success).
    pub status: String,
    /// The results of the operation.
    pub results: serde_json::Value,
    /// Response metadata (request ID, timing, etc.).
    pub metadata: CrudMetadata,
}

/// Metadata for CRUD responses.
#[derive(Debug, Serialize)]
pub struct CrudMetadata {
    /// Unique request identifier for tracking.
    pub request_id: String,
    /// Query execution time in milliseconds.
    pub execution_time_ms: u64,
    /// The TypeQL query that was executed.
    pub typeql: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_entity_insert_request() {
        let json = serde_json::json!({
            "attributes": {
                "name": { "value": "Alice", "value_type": "string" },
                "age": { "value": 30, "value_type": "long" }
            }
        });
        let req: EntityInsertRequest = serde_json::from_value(json).unwrap();
        assert!(req.database.is_none());
        assert_eq!(req.attributes.len(), 2);
        assert_eq!(req.attributes["name"].value_type, "string");
    }

    #[test]
    fn deserialize_entity_fetch_request_defaults() {
        let json = serde_json::json!({});
        let req: EntityFetchRequest = serde_json::from_value(json).unwrap();
        assert!(req.filters.is_empty());
        assert!(req.sort.is_empty());
        assert!(req.limit.is_none());
    }

    #[test]
    fn deserialize_relation_insert_request() {
        let json = serde_json::json!({
            "role_players": [
                {
                    "role": "employee",
                    "entity_type": "person",
                    "key_attr": "name",
                    "key_value": { "value": "Alice", "value_type": "string" }
                }
            ],
            "attributes": {}
        });
        let req: RelationInsertRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.role_players.len(), 1);
        assert_eq!(req.role_players[0].role, "employee");
    }

    #[test]
    fn deserialize_filter_spec() {
        let json = serde_json::json!({
            "attr": "age",
            "op": ">=",
            "value": { "value": 18, "value_type": "long" }
        });
        let filter: FilterSpec = serde_json::from_value(json).unwrap();
        assert_eq!(filter.attr, "age");
        assert_eq!(filter.op, ">=");
    }

    #[test]
    fn sort_spec_default_direction() {
        let json = serde_json::json!({ "attr": "name" });
        let sort: SortSpec = serde_json::from_value(json).unwrap();
        assert_eq!(sort.dir, "asc");
    }

    #[test]
    fn serialize_crud_response() {
        let resp = CrudResponse {
            status: "ok".to_string(),
            results: serde_json::json!({"inserted": true}),
            metadata: CrudMetadata {
                request_id: "abc-123".to_string(),
                execution_time_ms: 42,
                typeql: "insert $e isa person;".to_string(),
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["metadata"]["execution_time_ms"], 42);
        assert_eq!(json["metadata"]["typeql"], "insert $e isa person;");
    }

    #[test]
    fn role_player_spec_with_iid() {
        let json = serde_json::json!({
            "role": "friend",
            "entity_type": "person",
            "iid": "0xabc123"
        });
        let rp: RolePlayerSpec = serde_json::from_value(json).unwrap();
        assert_eq!(rp.iid.as_deref(), Some("0xabc123"));
        assert!(rp.key_attr.is_none());
    }

    #[test]
    fn attribute_value_spec_roundtrip() {
        let spec = AttributeValueSpec {
            value: serde_json::json!("hello"),
            value_type: "string".to_string(),
        };
        let json = serde_json::to_value(&spec).unwrap();
        let spec2: AttributeValueSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec2.value, serde_json::json!("hello"));
        assert_eq!(spec2.value_type, "string");
    }
}
