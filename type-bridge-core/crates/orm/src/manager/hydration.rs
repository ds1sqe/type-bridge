//! Hydration layer: converts TypeDB JSON results to typed Rust structs.
//!
//! TypeDB fetch queries return documents with nested attribute structures.
//! This module flattens those results and invokes
//! [`TypeBridgeEntity::from_document`] to produce typed entities.

use crate::descriptor::{EntityDescriptor, RelationDescriptor};
use crate::dynamic::{DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer};
use crate::entity::TypeBridgeEntity;
use crate::error::{OrmError, Result};
use crate::relation::TypeBridgeRelation;
use crate::session::backend::QueryResult;
use crate::value::AttributeValue;

/// Hydrate a single entity from a TypeDB fetch document.
///
/// Expected document shape from a polymorphic fetch:
/// ```json
/// {
///     "_iid": "0x...",
///     "_type": "person",
///     "attributes": {
///         "name": [{"value": "Alice", ...}],
///         "age": [{"value": 30, ...}]
///     }
/// }
/// ```
#[tracing::instrument(skip(doc), fields(entity_type = T::TYPE_NAME))]
pub fn hydrate_entity<T: TypeBridgeEntity>(doc: &serde_json::Value) -> Result<T> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: T::TYPE_NAME.to_string(),
        message: "Expected JSON object".into(),
    })?;

    // Extract IID (handles both scalar string and {"value": "..."} shapes)
    let iid = extract_scalar_string(obj, "_iid");

    // Extract and flatten attributes
    let flat = if let Some(attrs) = obj.get("attributes").and_then(|v| v.as_object()) {
        flatten_wildcard_attributes(attrs)
    } else {
        // No "attributes" wrapper — treat the document itself as flat
        // (skip metadata keys starting with '_')
        let mut flat = serde_json::Map::new();
        for (k, v) in obj {
            if !k.starts_with('_') && k != "attributes" {
                flat.insert(k.clone(), v.clone());
            }
        }
        flat
    };

    let mut entity = T::from_document(&flat)?;
    if let Some(iid) = iid {
        entity.set_iid(iid);
    }
    Ok(entity)
}

/// Hydrate a single relation from a TypeDB fetch document.
///
/// Uses the same document shape as entity hydration since relations
/// also own attributes and have IIDs.
#[tracing::instrument(skip(doc), fields(relation_type = R::TYPE_NAME))]
pub fn hydrate_relation<R: TypeBridgeRelation>(doc: &serde_json::Value) -> Result<R> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: R::TYPE_NAME.to_string(),
        message: "Expected JSON object".into(),
    })?;

    let iid = extract_scalar_string(obj, "_iid");

    let flat = if let Some(attrs) = obj.get("attributes").and_then(|v| v.as_object()) {
        flatten_wildcard_attributes(attrs)
    } else {
        let mut flat = serde_json::Map::new();
        for (k, v) in obj {
            if !k.starts_with('_') && k != "attributes" {
                flat.insert(k.clone(), v.clone());
            }
        }
        flat
    };

    let mut relation = R::from_document(&flat)?;
    if let Some(iid) = iid {
        relation.set_iid(iid);
    }
    Ok(relation)
}

/// Hydrate a dynamic entity row from a TypeDB fetch document.
#[tracing::instrument(skip(doc, descriptor), fields(entity_type = %descriptor.type_name))]
pub fn hydrate_dynamic_entity(
    descriptor: &EntityDescriptor,
    doc: &serde_json::Value,
) -> Result<DynamicEntityRow> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: descriptor.type_name.clone(),
        message: "Expected JSON object".into(),
    })?;

    let attributes =
        dynamic_attributes_from_document(&descriptor.type_name, &descriptor.owned_attributes, obj)?;

    Ok(DynamicEntityRow {
        iid: extract_scalar_string(obj, "_iid"),
        type_name: extract_scalar_string(obj, "_type"),
        attributes,
    })
}

/// Hydrate a dynamic relation row from a TypeDB fetch document.
#[tracing::instrument(skip(doc, descriptor), fields(relation_type = %descriptor.type_name))]
pub fn hydrate_dynamic_relation(
    descriptor: &RelationDescriptor,
    doc: &serde_json::Value,
) -> Result<DynamicRelationRow> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: descriptor.type_name.clone(),
        message: "Expected JSON object".into(),
    })?;

    let attributes =
        dynamic_attributes_from_document(&descriptor.type_name, &descriptor.owned_attributes, obj)?;

    Ok(DynamicRelationRow {
        iid: extract_scalar_string(obj, "_iid"),
        type_name: extract_scalar_string(obj, "_type"),
        attributes,
        role_players: hydrate_dynamic_role_players(descriptor, obj),
    })
}

/// Flatten TypeDB wildcard attribute results.
///
/// Input: `{ "name": [{"value": "Alice", ...}], "age": [{"value": 30, ...}] }`
/// Output: `{ "name": "Alice", "age": 30 }`
///
/// For each attribute, unwraps document scalar wrappers. Single values are
/// returned as scalars and repeated values are returned as arrays.
/// If the value is already flat (not an array), passes it through unchanged.
pub fn flatten_wildcard_attributes(
    attrs: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut flat = serde_json::Map::new();
    for (key, value) in attrs {
        if let Some(arr) = value.as_array() {
            let values: Vec<_> = arr.iter().map(unwrap_document_value).collect();
            match values.as_slice() {
                [] => {}
                [single] => {
                    flat.insert(key.clone(), single.clone());
                }
                _ => {
                    flat.insert(key.clone(), serde_json::Value::Array(values));
                }
            }
        } else {
            flat.insert(key.clone(), unwrap_document_value(value));
        }
    }
    flat
}

fn unwrap_document_value(value: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = value.as_object() else {
        return value.clone();
    };
    if let Some(inner) = obj.get("value") {
        return unwrap_document_value(inner);
    }
    for key in [
        "string",
        "long",
        "integer",
        "double",
        "boolean",
        "date",
        "datetime",
        "datetime-tz",
        "decimal",
        "duration",
    ] {
        if let Some(inner) = obj.get(key) {
            return unwrap_document_value(inner);
        }
    }
    value.clone()
}

fn flatten_document_attributes(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    if let Some(attrs) = obj.get("attributes").and_then(|v| v.as_object()) {
        flatten_wildcard_attributes(attrs)
    } else {
        let mut flat = serde_json::Map::new();
        for (key, value) in obj {
            if !key.starts_with('_') && key != "attributes" && key != "role_players" {
                flat.insert(key.clone(), value.clone());
            }
        }
        flat
    }
}

fn dynamic_attributes(
    type_name: &str,
    descriptors: &[crate::descriptor::OwnedAttributeDescriptor],
    flat: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(String, AttributeValue)>> {
    let mut attributes = Vec::new();
    for descriptor in descriptors {
        let Some(value) = flat.get(&descriptor.attr_name) else {
            if descriptor.is_optional {
                continue;
            }
            return Err(OrmError::Hydration {
                type_name: type_name.to_string(),
                message: format!("missing attribute '{}'", descriptor.attr_name),
            });
        };
        let values = value
            .as_array()
            .map(|items| items.iter().collect::<Vec<_>>())
            .unwrap_or_else(|| vec![value]);
        for value in values {
            let attribute = AttributeValue::from_json(value, descriptor.value_type.as_str())
                .ok_or_else(|| OrmError::Hydration {
                    type_name: type_name.to_string(),
                    message: format!(
                        "attribute '{}' is not a {} value: {}",
                        descriptor.attr_name, descriptor.value_type, value
                    ),
                })?;
            attributes.push((descriptor.attr_name.clone(), attribute));
        }
    }
    Ok(attributes)
}

fn dynamic_attributes_from_document(
    type_name: &str,
    descriptors: &[crate::descriptor::OwnedAttributeDescriptor],
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(String, AttributeValue)>> {
    let Some(attrs) = obj.get("attributes").and_then(|value| value.as_object()) else {
        let flat = flatten_document_attributes(obj);
        return dynamic_attributes(type_name, descriptors, &flat);
    };

    let known_value_types: std::collections::HashMap<&str, &str> = descriptors
        .iter()
        .map(|descriptor| {
            (
                descriptor.attr_name.as_str(),
                descriptor.value_type.as_str(),
            )
        })
        .collect();

    let mut attributes = Vec::new();
    for descriptor in descriptors {
        let Some(value) = attrs.get(&descriptor.attr_name) else {
            if descriptor.is_optional {
                continue;
            }
            return Err(OrmError::Hydration {
                type_name: type_name.to_string(),
                message: format!("missing attribute '{}'", descriptor.attr_name),
            });
        };
        if dynamic_attribute_values(
            type_name,
            &descriptor.attr_name,
            value,
            Some(descriptor.value_type.as_str()),
            &mut attributes,
        )? == 0
            && !descriptor.is_optional
        {
            return Err(OrmError::Hydration {
                type_name: type_name.to_string(),
                message: format!("missing attribute '{}'", descriptor.attr_name),
            });
        }
    }

    for (attr_name, value) in attrs {
        if known_value_types.contains_key(attr_name.as_str()) {
            continue;
        }
        dynamic_attribute_values(type_name, attr_name, value, None, &mut attributes)?;
    }

    Ok(attributes)
}

fn dynamic_attribute_values(
    type_name: &str,
    attr_name: &str,
    value: &serde_json::Value,
    known_value_type: Option<&str>,
    attributes: &mut Vec<(String, AttributeValue)>,
) -> Result<usize> {
    let values = value
        .as_array()
        .map(|items| items.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| vec![value]);
    let mut parsed = 0;
    for value in values {
        let Some(value_type) = known_value_type
            .or_else(|| extract_attribute_value_type(value))
            .or_else(|| infer_attribute_value_type(value))
        else {
            return Err(OrmError::Hydration {
                type_name: type_name.to_string(),
                message: format!("attribute '{attr_name}' is missing value type metadata"),
            });
        };
        let attribute =
            AttributeValue::from_json(value, value_type).ok_or_else(|| OrmError::Hydration {
                type_name: type_name.to_string(),
                message: format!("attribute '{attr_name}' is not a {value_type} value: {value}"),
            })?;
        attributes.push((attr_name.to_string(), attribute));
        parsed += 1;
    }
    Ok(parsed)
}

fn extract_attribute_value_type(value: &serde_json::Value) -> Option<&str> {
    let obj = value.as_object()?;
    obj.get("value_type")
        .and_then(|value| value.as_str())
        .or_else(|| {
            obj.get("type")
                .and_then(|value| value.as_object())
                .and_then(|value| value.get("value_type"))
                .and_then(|value| value.as_str())
        })
}

fn infer_attribute_value_type(value: &serde_json::Value) -> Option<&'static str> {
    let value = unwrap_document_value(value);
    if value.as_bool().is_some() {
        return Some("boolean");
    }
    if value.as_i64().is_some() {
        return Some("long");
    }
    if value.as_f64().is_some() {
        return Some("double");
    }
    if value.as_str().is_some() {
        return Some("string");
    }
    None
}

fn hydrate_dynamic_role_players(
    descriptor: &RelationDescriptor,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<DynamicRolePlayer> {
    let mut role_players: Vec<DynamicRolePlayer> = obj
        .get("role_players")
        .and_then(|value| value.as_array())
        .map(|players| {
            players
                .iter()
                .filter_map(|player| {
                    let player = player.as_object()?;
                    let role_name = extract_scalar_string(player, "role_name")
                        .or_else(|| extract_scalar_string(player, "role"))?;
                    Some(DynamicRolePlayer {
                        role_name,
                        player_iid: extract_scalar_string(player, "player_iid")
                            .or_else(|| extract_scalar_string(player, "iid")),
                        player_type_name: extract_scalar_string(player, "player_type_name")
                            .or_else(|| extract_scalar_string(player, "type_name")),
                        attributes: player
                            .get("attributes")
                            .and_then(|value| value.as_object())
                            .map(raw_attribute_entries)
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    for (index, role) in descriptor.roles.iter().enumerate() {
        let Some(player_iid) = extract_scalar_string(obj, &format!("_role_{index}_iid")) else {
            continue;
        };
        let attributes = obj
            .get(&format!("_role_{index}_attributes"))
            .and_then(|value| value.as_object())
            .map(flatten_wildcard_attributes)
            .map(|attributes| attributes.into_iter().collect())
            .unwrap_or_default();
        role_players.push(DynamicRolePlayer {
            role_name: role.role_name.clone(),
            player_iid: Some(player_iid),
            player_type_name: extract_scalar_string(obj, &format!("_role_{index}_type")),
            attributes,
        });
    }

    role_players
}

fn raw_attribute_entries(
    attrs: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    attrs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Extract a count value from a reduce query result.
///
/// Expects `QueryResult::Rows` with at least one row containing a
/// numeric `$count` (or `count`) field.
pub fn extract_count(result: &QueryResult) -> Result<u64> {
    match result {
        QueryResult::Rows(rows) => {
            let row = rows.first().ok_or_else(|| OrmError::Hydration {
                type_name: "count".into(),
                message: "No rows returned from count query".into(),
            })?;
            let obj = row.as_object().ok_or_else(|| OrmError::Hydration {
                type_name: "count".into(),
                message: "Expected row object".into(),
            })?;

            // Try standard variable names first
            if let Some(v) = obj.get("$count").or_else(|| obj.get("count")) {
                return parse_count_value(v);
            }
            // Fallback: first numeric value in the row
            for v in obj.values() {
                if let Ok(count) = parse_count_value(v) {
                    return Ok(count);
                }
            }
            Err(OrmError::Hydration {
                type_name: "count".into(),
                message: "No numeric count value found in result".into(),
            })
        }
        QueryResult::Ok => Err(OrmError::Hydration {
            type_name: "count".into(),
            message: "Expected Rows result for count query, got Ok".into(),
        }),
        QueryResult::Documents(_) => Err(OrmError::Hydration {
            type_name: "count".into(),
            message: "Expected Rows result for count query, got Documents".into(),
        }),
    }
}

/// Extract a string value from a document key.
///
/// Handles both scalar strings (`"0x123"`) and wrapped objects
/// (`{"value": "0x123"}`).
pub(crate) fn extract_scalar_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    let val = obj.get(key)?;
    if let Some(s) = val.as_str() {
        return Some(s.to_string());
    }
    if let Some(inner) = val.as_object()
        && let Some(s) = inner.get("value").and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    None
}

fn parse_count_value(v: &serde_json::Value) -> Result<u64> {
    let v = unwrap_document_value(v);
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    if let Some(n) = v.as_i64() {
        return Ok(n as u64);
    }
    if let Some(n) = v.as_f64() {
        return Ok(n as u64);
    }
    Err(OrmError::Hydration {
        type_name: "count".into(),
        message: format!("Cannot parse count value: {v}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::descriptor::{EntityDescriptor, OwnedAttributeDescriptor};

    #[test]
    fn flatten_nested_attributes() {
        let input: serde_json::Value = serde_json::json!({
            "name": [{"value": "Alice", "type": {"label": "name", "value_type": "string"}}],
            "age": [{"value": 30, "type": {"label": "age", "value_type": "long"}}]
        });
        let flat = flatten_wildcard_attributes(input.as_object().unwrap());
        assert_eq!(flat.get("name").unwrap(), &serde_json::json!("Alice"));
        assert_eq!(flat.get("age").unwrap(), &serde_json::json!(30));
    }

    #[test]
    fn flatten_nested_wrapped_scalar_attributes() {
        let input: serde_json::Value = serde_json::json!({
            "age": [{"value": {"integer": 30}, "type": {"label": "age", "value_type": "long"}}]
        });
        let flat = flatten_wildcard_attributes(input.as_object().unwrap());
        assert_eq!(flat.get("age").unwrap(), &serde_json::json!(30));
    }

    #[test]
    fn flatten_repeated_attributes() {
        let input: serde_json::Value = serde_json::json!({
            "tag": [{"value": "alpha"}, {"value": "shared"}]
        });
        let flat = flatten_wildcard_attributes(input.as_object().unwrap());
        assert_eq!(
            flat.get("tag").unwrap(),
            &serde_json::json!(["alpha", "shared"])
        );
    }

    #[test]
    fn flatten_already_flat() {
        let input: serde_json::Value = serde_json::json!({
            "name": "Alice",
            "age": 30
        });
        let flat = flatten_wildcard_attributes(input.as_object().unwrap());
        assert_eq!(flat.get("name").unwrap(), &serde_json::json!("Alice"));
        assert_eq!(flat.get("age").unwrap(), &serde_json::json!(30));
    }

    #[test]
    fn flatten_empty_array_skips() {
        let input: serde_json::Value = serde_json::json!({
            "name": [{"value": "Alice"}],
            "optional": []
        });
        let flat = flatten_wildcard_attributes(input.as_object().unwrap());
        assert_eq!(flat.len(), 1);
        assert!(flat.get("optional").is_none());
    }

    #[test]
    fn dynamic_entity_hydration_keeps_wildcard_subtype_attributes() {
        let descriptor = EntityDescriptor {
            type_name: "artifact".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                field_name: "name".into(),
                attr_name: "ArtifactName".into(),
                value_type: ValueType::String,
                annotations: vec![],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            doc: None,
            meta: Default::default(),
        };
        let doc = serde_json::json!({
            "_iid": "0x123",
            "_type": "user_story",
            "attributes": {
                "ArtifactName": [{"value": "Login Feature", "value_type": "string"}],
                "Priority": [{"value": 1, "value_type": "long"}]
            }
        });

        let row = hydrate_dynamic_entity(&descriptor, &doc).unwrap();

        assert_eq!(row.type_name.as_deref(), Some("user_story"));
        assert_eq!(row.attributes.len(), 2);
        assert!(row.attributes.contains(&(
            "ArtifactName".into(),
            AttributeValue::String("Login Feature".into())
        )));
        assert!(
            row.attributes
                .contains(&("Priority".into(), AttributeValue::Long(1)))
        );
    }

    #[test]
    fn extract_count_from_rows() {
        let result = QueryResult::Rows(vec![serde_json::json!({"$count": 42})]);
        assert_eq!(extract_count(&result).unwrap(), 42);
    }

    #[test]
    fn extract_count_fallback_key() {
        let result = QueryResult::Rows(vec![serde_json::json!({"total": 7})]);
        assert_eq!(extract_count(&result).unwrap(), 7);
    }

    #[test]
    fn extract_count_from_wrapped_value() {
        let result = QueryResult::Rows(vec![serde_json::json!({
            "$count": {
                "category": "Value",
                "label": "integer",
                "value": 2,
                "value_type": "integer"
            }
        })]);
        assert_eq!(extract_count(&result).unwrap(), 2);
    }

    #[test]
    fn extract_count_from_documents_fails() {
        let result = QueryResult::Documents(vec![]);
        assert!(extract_count(&result).is_err());
    }

    #[test]
    fn extract_scalar_string_plain() {
        let mut obj = serde_json::Map::new();
        obj.insert("_iid".into(), serde_json::json!("0xabc"));
        assert_eq!(extract_scalar_string(&obj, "_iid"), Some("0xabc".into()));
    }

    #[test]
    fn extract_scalar_string_wrapped() {
        let mut obj = serde_json::Map::new();
        obj.insert("_iid".into(), serde_json::json!({"value": "0xdef"}));
        assert_eq!(extract_scalar_string(&obj, "_iid"), Some("0xdef".into()));
    }

    #[test]
    fn extract_scalar_string_missing() {
        let obj = serde_json::Map::new();
        assert_eq!(extract_scalar_string(&obj, "_iid"), None);
    }
}
