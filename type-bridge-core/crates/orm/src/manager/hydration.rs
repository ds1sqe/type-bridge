//! Hydration layer: converts TypeDB JSON results to typed Rust structs.
//!
//! TypeDB fetch queries return documents with nested attribute structures.
//! This module flattens those results and invokes
//! [`TypeBridgeEntity::from_document`] to produce typed entities.

use crate::descriptor::{EntityDescriptor, RelationDescriptor};
use crate::dynamic::{
    DynamicEntityIdentity, DynamicEntityRow, DynamicRelationIdentity, DynamicRelationRow,
    DynamicRolePlayer,
};
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
        for (k, v) in sorted_object_entries(obj) {
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
        for (k, v) in sorted_object_entries(obj) {
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

/// Hydrate mandatory identity-only evidence from a subtype discovery document.
pub fn hydrate_dynamic_entity_identity(
    root_type_name: &str,
    doc: &serde_json::Value,
) -> Result<DynamicEntityIdentity> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: root_type_name.to_string(),
        message: "Expected JSON object for entity identity discovery".into(),
    })?;
    let iid = extract_scalar_string(obj, "_iid").ok_or_else(|| OrmError::Hydration {
        type_name: root_type_name.to_string(),
        message: "Entity identity discovery omitted its IID".into(),
    })?;
    if !type_bridge_contract::id::is_canonical_thing_iid(&iid) {
        return Err(OrmError::Hydration {
            type_name: root_type_name.to_string(),
            message: "Entity identity discovery returned a noncanonical IID".into(),
        });
    }
    let type_name = extract_scalar_string(obj, "_type").ok_or_else(|| OrmError::Hydration {
        type_name: root_type_name.to_string(),
        message: "Entity identity discovery omitted its concrete type".into(),
    })?;
    if type_name.trim().is_empty() {
        return Err(OrmError::Hydration {
            type_name: root_type_name.to_string(),
            message: "Entity identity discovery returned a blank concrete type".into(),
        });
    }

    Ok(DynamicEntityIdentity { iid, type_name })
}

/// Hydrate identity-only relation discovery evidence.
pub fn hydrate_dynamic_relation_identity(
    root_type_name: &str,
    doc: &serde_json::Value,
) -> Result<DynamicRelationIdentity> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: root_type_name.to_string(),
        message: "Expected JSON object for relation identity discovery".into(),
    })?;
    let iid = relation_identity_scalar(obj, "_iid", root_type_name, "IID")?;
    if iid.trim().is_empty() {
        return Err(OrmError::Hydration {
            type_name: root_type_name.to_string(),
            message: "Relation identity discovery returned a blank IID".into(),
        });
    }
    if !type_bridge_contract::id::is_canonical_thing_iid(&iid) {
        return Err(OrmError::Hydration {
            type_name: root_type_name.to_string(),
            message: "Relation identity discovery returned a noncanonical IID".into(),
        });
    }
    let type_name = relation_identity_scalar(obj, "_type", root_type_name, "concrete type")?;
    if type_name.trim().is_empty() {
        return Err(OrmError::Hydration {
            type_name: root_type_name.to_string(),
            message: "Relation identity discovery returned a blank concrete type".into(),
        });
    }
    Ok(DynamicRelationIdentity { iid, type_name })
}

pub(crate) fn extract_scalar_identity(root: &str, doc: &serde_json::Value) -> Result<String> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: root.into(),
        message: "Player resolution returned a non-object".into(),
    })?;
    let value = obj.get("iid").ok_or_else(|| OrmError::Hydration {
        type_name: root.into(),
        message: "Player resolution omitted its IID".into(),
    })?;
    let scalar = value.get("value").unwrap_or(value);
    let iid = scalar
        .as_str()
        .ok_or_else(|| OrmError::Hydration {
            type_name: root.into(),
            message: "Player resolution returned a nonstring IID".into(),
        })?
        .to_string();
    if iid.trim().is_empty() {
        return Err(OrmError::Hydration {
            type_name: root.into(),
            message: "Player resolution returned a blank IID".into(),
        });
    }
    if !type_bridge_contract::id::is_canonical_thing_iid(&iid) {
        return Err(OrmError::Hydration {
            type_name: root.into(),
            message: "Player resolution returned a noncanonical IID".into(),
        });
    }
    Ok(iid)
}

fn relation_identity_scalar(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    root: &str,
    label: &str,
) -> Result<String> {
    let Some(value) = obj.get(key) else {
        return Err(OrmError::Hydration {
            type_name: root.to_string(),
            message: format!("Relation identity discovery omitted its {label}"),
        });
    };
    let scalar = value.get("value").unwrap_or(value);
    let Some(text) = scalar.as_str() else {
        return Err(OrmError::Hydration {
            type_name: root.to_string(),
            message: format!("Relation identity discovery returned a nonstring {label}"),
        });
    };
    Ok(text.to_string())
}

/// Hydrate one dynamic relation document through the compatibility surface.
#[tracing::instrument(skip(doc, descriptor), fields(relation_type = %descriptor.type_name))]
pub fn hydrate_dynamic_relation(
    descriptor: &RelationDescriptor,
    doc: &serde_json::Value,
) -> Result<DynamicRelationRow> {
    let mut row = hydrate_dynamic_relation_candidate(descriptor, doc)?;
    finalize_relation_players(descriptor, &mut row)?;
    Ok(row)
}

fn hydrate_dynamic_relation_candidate(
    descriptor: &RelationDescriptor,
    doc: &serde_json::Value,
) -> Result<DynamicRelationRow> {
    let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
        type_name: descriptor.type_name.clone(),
        message: "Expected JSON object".into(),
    })?;

    let attributes = dynamic_relation_attributes_from_document(descriptor, obj)?;

    let iid = extract_scalar_string(obj, "_iid");
    let type_name = extract_scalar_string(obj, "_type");
    validate_relation_identity(&descriptor.type_name, iid.as_deref(), type_name.as_deref())?;

    let role_players =
        normalize_candidate_players(descriptor, hydrate_dynamic_role_players(descriptor, obj)?)?;

    Ok(DynamicRelationRow {
        iid,
        type_name,
        attributes,
        role_players,
    })
}

/// Coalesce Cartesian relation-document sightings into one logical row per IID.
pub(crate) fn coalesce_dynamic_relations(
    descriptor: &RelationDescriptor,
    docs: &[serde_json::Value],
) -> Result<Vec<DynamicRelationRow>> {
    let mut groups: Vec<DynamicRelationRow> = Vec::new();
    for doc in docs {
        let candidate = hydrate_dynamic_relation_candidate(descriptor, doc)?;
        let iid = candidate.iid.as_deref().expect("validated relation IID");
        let Some(existing) = groups
            .iter_mut()
            .find(|row| row.iid.as_deref() == Some(iid))
        else {
            groups.push(candidate);
            continue;
        };
        if existing.type_name != candidate.type_name {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "contradictory relation type evidence",
            ));
        }
        if existing.attributes != candidate.attributes {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "contradictory relation attribute evidence",
            ));
        }
        merge_relation_players(descriptor, existing, candidate.role_players)?;
    }
    for row in &mut groups {
        finalize_relation_players(descriptor, row)?;
    }
    Ok(groups)
}

pub(crate) fn coalesce_dynamic_relation_by_iid(
    descriptor: &RelationDescriptor,
    docs: &[serde_json::Value],
    requested_iid: &str,
) -> Result<Vec<DynamicRelationRow>> {
    let rows = coalesce_dynamic_relations(descriptor, docs)?;
    if rows.len() > 1 {
        return Err(relation_hydration_error(
            &descriptor.type_name,
            "IID lookup returned multiple logical relations",
        ));
    }
    if let Some(row) = rows.first()
        && row.iid.as_deref() != Some(requested_iid)
    {
        return Err(relation_hydration_error(
            &descriptor.type_name,
            "IID lookup returned a different relation IID",
        ));
    }
    Ok(rows)
}

fn validate_relation_identity(
    type_name: &str,
    iid: Option<&str>,
    concrete_type: Option<&str>,
) -> Result<()> {
    let Some(iid) = iid.filter(|value| !value.trim().is_empty()) else {
        return Err(relation_hydration_error(
            type_name,
            "relation IID is missing or blank",
        ));
    };
    if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
        return Err(relation_hydration_error(
            type_name,
            "relation IID is not canonical",
        ));
    }
    if concrete_type.is_none_or(|value| value.trim().is_empty()) {
        return Err(relation_hydration_error(
            type_name,
            "relation concrete type is missing or blank",
        ));
    }
    Ok(())
}

fn relation_hydration_error(type_name: &str, message: &str) -> OrmError {
    OrmError::Hydration {
        type_name: type_name.to_string(),
        message: message.to_string(),
    }
}

fn validate_relation_attributes(
    descriptor: &RelationDescriptor,
    mut attributes: Vec<(String, AttributeValue)>,
) -> Result<Vec<(String, AttributeValue)>> {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for (name, _) in &attributes {
        *counts.entry(name.clone()).or_default() += 1;
    }
    for owned in &descriptor.owned_attributes {
        let count = counts.get(&owned.attr_name).copied().unwrap_or(0);
        if owned.cardinality().is_none() && count > 1 {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                &format!("duplicate scalar relation attribute '{}'", owned.attr_name),
            ));
        }
        let (min, max) = owned
            .cardinality()
            .unwrap_or((u32::from(!owned.is_optional), Some(1)));
        if count < min as usize || max.is_some_and(|limit| count > limit as usize) {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                &format!(
                    "relation attribute '{}' violates cardinality",
                    owned.attr_name
                ),
            ));
        }
    }
    attributes.sort_by_key(|(name, _)| {
        descriptor
            .owned_attributes
            .iter()
            .position(|owned| owned.attr_name == *name)
            .map(|index| (0_u8, index, name.clone()))
            .unwrap_or((1_u8, usize::MAX, name.clone()))
    });
    let mut offset = 0;
    while offset < attributes.len() {
        let name = attributes[offset].0.clone();
        let end = attributes[offset..]
            .iter()
            .position(|(candidate, _)| *candidate != name)
            .map(|index| offset + index)
            .unwrap_or(attributes.len());
        let ordered = descriptor
            .owned_attributes
            .iter()
            .find(|owned| owned.attr_name == name)
            .is_some_and(|owned| owned.is_ordered);
        if !ordered {
            attributes[offset..end].sort_by_key(|(_, value)| {
                serde_json::to_string(value).unwrap_or_else(|_| format!("{:?}", value))
            });
        }
        offset = end;
    }
    Ok(attributes)
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
    for (key, value) in sorted_object_entries(attrs) {
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
        for (key, value) in sorted_object_entries(obj) {
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

fn dynamic_relation_attributes_from_document(
    descriptor: &RelationDescriptor,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(String, AttributeValue)>> {
    let owned = obj
        .get("attributes")
        .map(|value| {
            value.as_object().ok_or_else(|| {
                relation_hydration_error(
                    &descriptor.type_name,
                    "relation attributes must be an object",
                )
            })
        })
        .transpose()?;
    let flattened;
    let attrs = if let Some(attrs) = owned {
        attrs
    } else {
        flattened = flatten_document_attributes(obj);
        &flattened
    };
    let mut parsed = Vec::new();
    for field in &descriptor.owned_attributes {
        let min = field
            .cardinality()
            .map(|(min, _)| min)
            .unwrap_or(u32::from(!field.is_optional));
        let Some(value) = attrs.get(&field.attr_name) else {
            if min == 0 {
                continue;
            }
            return Err(relation_hydration_error(
                &descriptor.type_name,
                &format!("missing attribute '{}'", field.attr_name),
            ));
        };
        let count = dynamic_attribute_values(
            &descriptor.type_name,
            &field.attr_name,
            value,
            Some(field.value_type.as_str()),
            &mut parsed,
        )?;
        if count < min as usize {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                &format!("missing attribute '{}'", field.attr_name),
            ));
        }
    }
    for (name, value) in sorted_object_entries(attrs) {
        if descriptor
            .owned_attributes
            .iter()
            .any(|owned| owned.attr_name == *name)
        {
            continue;
        }
        dynamic_attribute_values(&descriptor.type_name, name, value, None, &mut parsed)?;
    }
    validate_relation_attributes(descriptor, parsed)
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
) -> Result<Vec<DynamicRolePlayer>> {
    let mut role_players = Vec::new();
    for key in obj.keys().filter(|key| key.starts_with("_role_")) {
        let Some(rest) = key.strip_prefix("_role_") else {
            continue;
        };
        let mut parts = rest.split('_');
        let Some(index_text) = parts.next() else {
            continue;
        };
        let Some(suffix) = parts.next() else {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "malformed indexed relation role",
            ));
        };
        if parts.next().is_some() || !matches!(suffix, "iid" | "type" | "attributes") {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "malformed indexed relation role",
            ));
        }
        let Some(index) = index_text.parse::<usize>().ok() else {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "malformed indexed relation role",
            ));
        };
        if index_text != index.to_string() {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "malformed indexed relation role",
            ));
        }
        if index >= descriptor.roles.len() {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "unknown indexed relation role",
            ));
        }
    }
    if let Some(value) = obj.get("role_players") {
        let players = value.as_array().ok_or_else(|| {
            relation_hydration_error(&descriptor.type_name, "malformed generic player evidence")
        })?;
        for player in players {
            let player = player.as_object().ok_or_else(|| {
                relation_hydration_error(&descriptor.type_name, "malformed generic player evidence")
            })?;
            let role_name = extract_alias_string(
                &descriptor.type_name,
                player,
                "role_name",
                "role",
                "player role is missing",
            )?;
            let role = descriptor.role(&role_name).ok_or_else(|| {
                relation_hydration_error(&descriptor.type_name, "unknown relation role")
            })?;
            let iid = extract_alias_string(
                &descriptor.type_name,
                player,
                "player_iid",
                "iid",
                "player IID is missing",
            )?;
            let player_type_name = extract_alias_string(
                &descriptor.type_name,
                player,
                "player_type_name",
                "type_name",
                "player concrete type is missing",
            )?;
            validate_player_identity(&descriptor.type_name, &iid, &player_type_name)?;
            let attributes = normalize_player_attributes(
                &descriptor.type_name,
                player.get("attributes"),
                "generic player attributes must be an object",
            )?;
            role_players.push(DynamicRolePlayer {
                role_name: role.role_name.clone(),
                player_iid: Some(iid),
                player_type_name: Some(player_type_name),
                attributes,
            });
        }
    }

    for (index, role) in descriptor.roles.iter().enumerate() {
        let iid_key = format!("_role_{index}_iid");
        let type_key = format!("_role_{index}_type");
        let attrs_key = format!("_role_{index}_attributes");
        let iid_present = obj.contains_key(&iid_key);
        let type_present = obj.contains_key(&type_key);
        let attrs_present = obj.contains_key(&attrs_key);
        let iid = extract_scalar_string(obj, &iid_key);
        let concrete_type = extract_scalar_string(obj, &type_key);
        let indexed_attributes = obj.get(&attrs_key);
        let any_indexed = iid_present || type_present || attrs_present;
        if any_indexed && !(iid_present && type_present && attrs_present) {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "partial indexed role evidence",
            ));
        }
        let empty_optional_player = iid.is_none()
            && concrete_type.is_none()
            && indexed_attributes.is_some_and(|attributes| {
                attributes.is_null()
                    || attributes
                        .as_object()
                        .is_some_and(serde_json::Map::is_empty)
            });
        if empty_optional_player {
            continue;
        }
        if any_indexed && (iid.is_none() || concrete_type.is_none()) {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "indexed player IID/type must be strings",
            ));
        }
        let Some(player_iid) = iid else {
            continue;
        };
        validate_player_identity(
            &descriptor.type_name,
            &player_iid,
            concrete_type.as_deref().unwrap(),
        )?;
        let attributes_object = indexed_attributes.unwrap().as_object().ok_or_else(|| {
            relation_hydration_error(
                &descriptor.type_name,
                "indexed player attributes must be an object",
            )
        })?;
        let attributes = normalize_player_attributes(
            &descriptor.type_name,
            Some(&serde_json::Value::Object(attributes_object.clone())),
            "indexed player attributes must be an object",
        )?;
        role_players.push(DynamicRolePlayer {
            role_name: role.role_name.clone(),
            player_iid: Some(player_iid),
            player_type_name: concrete_type,
            attributes,
        });
    }

    Ok(role_players)
}

fn extract_alias_string(
    type_name: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    primary: &str,
    alias: &str,
    missing: &str,
) -> Result<String> {
    let primary_value = object.get(primary);
    let alias_value = object.get(alias);
    if primary_value.is_none() && alias_value.is_none() {
        return Err(relation_hydration_error(type_name, missing));
    }
    let parse = |value: &serde_json::Value| {
        value.as_str().map(str::to_owned).or_else(|| {
            value
                .get("value")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
    };
    let primary_text = primary_value.and_then(parse);
    let alias_text = alias_value.and_then(parse);
    if primary_value.is_some() && primary_text.is_none()
        || alias_value.is_some() && alias_text.is_none()
    {
        return Err(relation_hydration_error(
            type_name,
            &format!("malformed player {primary}/{alias} evidence"),
        ));
    }
    if let (Some(primary_text), Some(alias_text)) = (&primary_text, &alias_text)
        && primary_text != alias_text
    {
        return Err(relation_hydration_error(
            type_name,
            &format!("contradictory player {primary}/{alias} evidence"),
        ));
    }
    Ok(primary_text.or(alias_text).unwrap())
}

fn normalize_player_attributes(
    type_name: &str,
    value: Option<&serde_json::Value>,
    malformed: &str,
) -> Result<Vec<(String, serde_json::Value)>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| relation_hydration_error(type_name, malformed))?;
    Ok(canonical_json_entries(
        flatten_wildcard_attributes(object).into_iter().collect(),
    ))
}

fn validate_player_identity(type_name: &str, iid: &str, concrete_type: &str) -> Result<()> {
    if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
        return Err(relation_hydration_error(
            type_name,
            "player IID is not canonical",
        ));
    }
    if concrete_type.trim().is_empty() {
        return Err(relation_hydration_error(
            type_name,
            "player concrete type is blank",
        ));
    }
    Ok(())
}

fn canonical_json_entries(
    mut entries: Vec<(String, serde_json::Value)>,
) -> Vec<(String, serde_json::Value)> {
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (_, value) in &mut entries {
        canonical_json(value);
    }
    entries
}

fn canonical_json(value: &mut serde_json::Value) {
    if let Some(object) = value.as_object_mut() {
        let mut ordered = serde_json::Map::new();
        let mut keys: Vec<_> = object.keys().cloned().collect();
        keys.sort();
        for key in keys {
            if let Some(mut value) = object.remove(&key) {
                canonical_json(&mut value);
                ordered.insert(key, value);
            }
        }
        *object = ordered;
    } else if let Some(array) = value.as_array_mut() {
        for item in &mut *array {
            canonical_json(item);
        }
        array.sort_by_key(|item| item.to_string());
    }
}

fn normalize_candidate_players(
    descriptor: &RelationDescriptor,
    players: Vec<DynamicRolePlayer>,
) -> Result<Vec<DynamicRolePlayer>> {
    let mut normalized = Vec::new();
    for player in players {
        let iid = player.player_iid.as_deref().unwrap();
        if let Some(existing) = normalized
            .iter()
            .find(|candidate: &&DynamicRolePlayer| candidate.player_iid.as_deref() == Some(iid))
        {
            if existing.player_type_name != player.player_type_name {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player type evidence",
                ));
            }
            if existing.attributes != player.attributes {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player attribute evidence",
                ));
            }
        }
        if let Some(existing) = normalized.iter().find(|candidate| {
            candidate.role_name == player.role_name && candidate.player_iid == player.player_iid
        }) {
            if existing.player_type_name != player.player_type_name {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player type evidence",
                ));
            }
            if existing.attributes != player.attributes {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player attribute evidence",
                ));
            }
            continue;
        }
        normalized.push(player);
    }
    Ok(normalized)
}

fn merge_relation_players(
    descriptor: &RelationDescriptor,
    existing: &mut DynamicRelationRow,
    incoming: Vec<DynamicRolePlayer>,
) -> Result<()> {
    for player in incoming {
        let iid = player.player_iid.as_deref().unwrap();
        if let Some(other) = existing
            .role_players
            .iter()
            .find(|candidate| candidate.player_iid.as_deref() == Some(iid))
        {
            if other.player_type_name != player.player_type_name {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player type evidence",
                ));
            }
            if other.attributes != player.attributes {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player attribute evidence",
                ));
            }
        }
        if let Some(other) = existing.role_players.iter().find(|candidate| {
            candidate.role_name == player.role_name && candidate.player_iid == player.player_iid
        }) {
            if other.player_type_name != player.player_type_name {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player type evidence",
                ));
            }
            if other.attributes != player.attributes {
                return Err(relation_hydration_error(
                    &descriptor.type_name,
                    "contradictory player attribute evidence",
                ));
            }
        } else {
            existing.role_players.push(player);
        }
    }
    Ok(())
}

fn finalize_relation_players(
    descriptor: &RelationDescriptor,
    row: &mut DynamicRelationRow,
) -> Result<()> {
    for role in &descriptor.roles {
        let mut players: Vec<_> = row
            .role_players
            .iter()
            .filter(|player| player.role_name == role.role_name)
            .cloned()
            .collect();
        let (min, max) = role.cardinality.unwrap_or((0, None));
        if players.len() < min as usize || max.is_some_and(|limit| players.len() > limit as usize) {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "relation role violates cardinality",
            ));
        }
        if role.ordered && players.len() > 1 {
            return Err(relation_hydration_error(
                &descriptor.type_name,
                "ordered role lacks list-order evidence",
            ));
        }
        if !role.ordered {
            players.sort_by(|left, right| {
                left.player_type_name
                    .cmp(&right.player_type_name)
                    .then_with(|| left.player_iid.cmp(&right.player_iid))
            });
        }
        row.role_players
            .retain(|player| player.role_name != role.role_name);
        row.role_players.extend(players);
    }
    Ok(())
}

fn sorted_object_entries(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(&String, &serde_json::Value)> {
    let mut entries: Vec<_> = object.iter().collect();
    entries.sort_unstable_by_key(|(key, _)| *key);
    entries
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
    use crate::descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };
    use crate::entity::Annotation;

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

    fn relation_fixture() -> RelationDescriptor {
        RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                field_name: "position".into(),
                attr_name: "position".into(),
                value_type: ValueType::String,
                annotations: vec![],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            roles: vec![
                RoleDescriptor {
                    role_name: "employee".into(),
                    cardinality: Some((1, None)),
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "employer".into(),
                    cardinality: Some((1, None)),
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        }
    }

    fn cartesian_doc(employee: &str, employer: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": "0xabc",
            "_type": "employment",
            "attributes": {"position": [{"value": "Engineer"}]},
            "_role_0_iid": employee,
            "_role_0_type": "person",
            "_role_0_attributes": {"name": [{"value": "Alice"}]},
            "_role_1_iid": employer,
            "_role_1_type": "company",
            "_role_1_attributes": {"name": [{"value": "Acme"}]}
        })
    }

    #[test]
    fn relation_documents_coalesce_cartesian_sightings_in_role_order() {
        let descriptor = relation_fixture();
        let docs = vec![
            cartesian_doc("0x102", "0x202"),
            cartesian_doc("0x101", "0x202"),
            cartesian_doc("0x102", "0x201"),
            cartesian_doc("0x101", "0x201"),
        ];
        let rows = coalesce_dynamic_relations(&descriptor, &docs).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].iid.as_deref(), Some("0xabc"));
        assert_eq!(rows[0].role_players.len(), 4);
        assert_eq!(
            rows[0]
                .role_players
                .iter()
                .map(|player| (player.role_name.as_str(), player.player_iid.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("employee", Some("0x101")),
                ("employee", Some("0x102")),
                ("employer", Some("0x201")),
                ("employer", Some("0x202")),
            ]
        );
    }

    #[test]
    fn relation_documents_reject_contradictory_and_malformed_evidence() {
        let descriptor = relation_fixture();
        let first = cartesian_doc("0x101", "0x201");
        let mut contradictory = cartesian_doc("0x101", "0x201");
        contradictory["_type"] = serde_json::json!("contract");
        assert!(coalesce_dynamic_relations(&descriptor, &[first, contradictory]).is_err());

        let mut malformed = cartesian_doc("0x101", "0x201");
        malformed["_role_0_iid"] = serde_json::json!("person-1");
        let error = coalesce_dynamic_relations(&descriptor, &[malformed]).unwrap_err();
        assert!(
            matches!(error, OrmError::Hydration { message, .. } if message.contains("canonical"))
        );
    }

    #[test]
    fn relation_documents_reject_duplicate_scalar_and_partial_role_evidence() {
        let mut descriptor = relation_fixture();
        let mut duplicate = cartesian_doc("0x101", "0x201");
        duplicate["attributes"]["position"] = serde_json::json!([
            {"value": "Engineer"},
            {"value": "Engineer"}
        ]);
        assert!(coalesce_dynamic_relations(&descriptor, &[duplicate]).is_err());

        descriptor.roles[1].cardinality = Some((0, None));
        let mut partial = cartesian_doc("0x101", "0x201");
        partial.as_object_mut().unwrap().remove("_role_1_type");
        assert!(coalesce_dynamic_relations(&descriptor, &[partial]).is_err());
    }

    #[test]
    fn relation_by_iid_coalesces_cartesian_duplicates_and_rejects_multiple_iids() {
        let descriptor = relation_fixture();
        let docs = vec![
            cartesian_doc("0x101", "0x201"),
            cartesian_doc("0x102", "0x201"),
        ];
        let row = coalesce_dynamic_relation_by_iid(&descriptor, &docs, "0xabc").unwrap();
        assert_eq!(row.len(), 1);
        let mut other = cartesian_doc("0x101", "0x201");
        other["_iid"] = serde_json::json!("0xdef");
        let error = coalesce_dynamic_relation_by_iid(
            &descriptor,
            &[cartesian_doc("0x101", "0x201"), other],
            "0xabc",
        )
        .unwrap_err();
        assert!(
            matches!(error, OrmError::Hydration { message, .. } if message.contains("multiple logical"))
        );
    }

    #[test]
    fn relation_role_minimum_is_checked_after_cartesian_grouping() {
        let mut descriptor = relation_fixture();
        descriptor.roles[0].cardinality = Some((2, None));
        descriptor.roles[1].cardinality = Some((2, None));
        let rows = coalesce_dynamic_relations(
            &descriptor,
            &[
                cartesian_doc("0x101", "0x201"),
                cartesian_doc("0x102", "0x202"),
            ],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role_players.len(), 4);
    }

    #[test]
    fn single_relation_wrapper_finalizes_cardinality_and_order() {
        let descriptor = relation_fixture();
        let row = hydrate_dynamic_relation(&descriptor, &cartesian_doc("0x101", "0x201")).unwrap();
        assert_eq!(row.role_players.len(), 2);
        let mut ordered = descriptor.clone();
        ordered.roles[0].ordered = true;
        ordered.roles[0].cardinality = Some((0, None));
        let mut second = cartesian_doc("0x102", "0x201");
        second["_iid"] = serde_json::json!("0xdef");
        assert!(hydrate_dynamic_relation(&ordered, &second).is_ok());
        let mut duplicate = cartesian_doc("0x101", "0x201");
        duplicate["role_players"] = serde_json::json!([
            {"role_name":"employee","player_iid":"0x101","player_type_name":"person","attributes":{"name":["B","A"]}},
            {"role_name":"employee","player_iid":"0x101","player_type_name":"person","attributes":{"name":["A","B"]}},
            {"role_name":"employer","player_iid":"0x201","player_type_name":"company","attributes":{"name":["Acme"]}},
        ]);
        duplicate
            .as_object_mut()
            .unwrap()
            .retain(|key, _| !key.starts_with("_role_"));
        assert!(hydrate_dynamic_relation(&descriptor, &duplicate).is_ok());
        let mut many = cartesian_doc("0x101", "0x201");
        many.as_object_mut()
            .unwrap()
            .retain(|key, _| !key.starts_with("_role_"));
        many["role_players"] = serde_json::json!([
            {"role_name":"employee","player_iid":"0x101","player_type_name":"person","attributes":{"name":["Alice"]}},
            {"role_name":"employee","player_iid":"0x102","player_type_name":"person","attributes":{"name":["Alice"]}},
            {"role_name":"employer","player_iid":"0x201","player_type_name":"company","attributes":{"name":["Acme"]}}
        ]);
        let mut ordered_many = ordered.clone();
        ordered_many.roles[0].cardinality = Some((0, None));
        let error = hydrate_dynamic_relation(&ordered_many, &many).unwrap_err();
        assert!(
            matches!(error, OrmError::Hydration { message, .. } if message == "ordered role lacks list-order evidence")
        );
    }

    #[test]
    fn relation_player_type_and_attribute_conflicts_are_distinct() {
        let descriptor = relation_fixture();
        let mut type_conflict = cartesian_doc("0x101", "0x201");
        type_conflict["_role_0_type"] = serde_json::json!("company");
        let err = coalesce_dynamic_relations(
            &descriptor,
            &[cartesian_doc("0x101", "0x201"), type_conflict],
        )
        .unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { message, .. } if message == "contradictory player type evidence")
        );
        let mut attr_conflict = cartesian_doc("0x101", "0x201");
        attr_conflict["_role_0_attributes"]["name"] = serde_json::json!([{"value":"Bob"}]);
        let err = coalesce_dynamic_relations(
            &descriptor,
            &[cartesian_doc("0x101", "0x201"), attr_conflict],
        )
        .unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { message, .. } if message == "contradictory player attribute evidence")
        );
    }

    #[test]
    fn relation_indexed_presence_rejects_malformed_and_out_of_range_keys() {
        let descriptor = relation_fixture();
        for key in ["_role_0_bad", "_role_x_iid", "_role_2_iid"] {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc[key] = serde_json::json!("0x999");
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            assert!(
                matches!(err, OrmError::Hydration { message, .. } if message == "malformed indexed relation role" || message == "unknown indexed relation role")
            );
        }
        let mut attrs_only = cartesian_doc("0x101", "0x201");
        attrs_only.as_object_mut().unwrap().remove("_role_0_iid");
        attrs_only.as_object_mut().unwrap().remove("_role_0_type");
        let err = coalesce_dynamic_relations(&descriptor, &[attrs_only]).unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { message, .. } if message == "partial indexed role evidence")
        );
        let mut wrong_shapes = cartesian_doc("0x101", "0x201");
        wrong_shapes["_role_0_iid"] = serde_json::json!(42);
        wrong_shapes["_role_0_type"] = serde_json::json!(42);
        wrong_shapes
            .as_object_mut()
            .unwrap()
            .remove("_role_0_attributes");
        let err = coalesce_dynamic_relations(&descriptor, &[wrong_shapes]).unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { message, .. } if message == "partial indexed role evidence")
        );
    }

    #[test]
    fn relation_adversarial_identity_player_and_container_matrix_is_specific() {
        let descriptor = relation_fixture();
        let valid = || cartesian_doc("0x101", "0x201");
        let mut cases: Vec<(&str, serde_json::Value, &str)> = Vec::new();
        let mut doc = valid();
        doc["_iid"] = serde_json::Value::Null;
        cases.push((
            "missing relation iid",
            doc,
            "relation IID is missing or blank",
        ));
        let mut doc = valid();
        doc["_iid"] = serde_json::json!("bad");
        cases.push((
            "noncanonical relation iid",
            doc,
            "relation IID is not canonical",
        ));
        let mut doc = valid();
        doc["_type"] = serde_json::json!(" ");
        cases.push((
            "blank relation type",
            doc,
            "relation concrete type is missing or blank",
        ));
        let mut doc = valid();
        doc["_type"] = serde_json::json!(42);
        cases.push((
            "nonstring relation type",
            doc,
            "relation concrete type is missing or blank",
        ));
        let mut doc = valid();
        doc["_role_0_iid"] = serde_json::json!(" ");
        cases.push(("blank player iid", doc, "player IID is not canonical"));
        let mut doc = valid();
        doc["_role_0_iid"] = serde_json::json!("bad");
        cases.push((
            "noncanonical player iid",
            doc,
            "player IID is not canonical",
        ));
        let mut doc = valid();
        doc["_role_0_type"] = serde_json::json!(" ");
        cases.push(("blank player type", doc, "player concrete type is blank"));
        let mut doc = valid();
        doc["_role_0_type"] = serde_json::json!(42);
        cases.push((
            "nonstring player type",
            doc,
            "indexed player IID/type must be strings",
        ));
        let mut doc = valid();
        doc["role_players"] = serde_json::json!("bad");
        cases.push((
            "malformed generic container",
            doc,
            "malformed generic player evidence",
        ));
        let mut doc = valid();
        doc["role_players"] = serde_json::json!([1]);
        cases.push((
            "malformed generic member",
            doc,
            "malformed generic player evidence",
        ));
        let mut doc = valid();
        doc["role_players"] = serde_json::json!([{"role_name":"unknown","player_iid":"0x101","player_type_name":"person"}]);
        cases.push(("unknown generic role", doc, "unknown relation role"));
        let mut doc = valid();
        doc["role_players"] = serde_json::json!([{"role_name":"employee","player_iid":"0x101","player_type_name":"person","attributes":[]}]);
        cases.push((
            "nonobject generic attributes",
            doc,
            "generic player attributes must be an object",
        ));
        for (name, doc, reason) in cases {
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            assert!(
                matches!(&err, OrmError::Hydration { message, .. } if message == reason),
                "{name}: {err:?}"
            );
        }
    }

    #[test]
    fn relation_attributes_use_exact_names_and_deterministic_unordered_values() {
        let mut descriptor = relation_fixture();
        descriptor.owned_attributes[0].attr_name = "position-key".into();
        descriptor.owned_attributes[0].field_name = "position".into();
        let mut first = cartesian_doc("0x101", "0x201");
        first["attributes"] = serde_json::json!({
            "position-key": [{"value":"B"}],
            "position": [{"value":"field-alias"}],
            "zeta": [{"value":"2"},{"value":"1"}]
        });
        let mut second = first.clone();
        second["attributes"]["position-key"] = serde_json::json!([{"value":"B"}]);
        second["attributes"]["zeta"] = serde_json::json!([{"value":"1"},{"value":"2"}]);
        let rows = coalesce_dynamic_relations(&descriptor, &[first, second]).unwrap();
        assert_eq!(rows.len(), 1);
        let names = rows[0]
            .attributes
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["position-key", "position", "zeta", "zeta"]);
        assert_eq!(rows[0].attributes[0].1, AttributeValue::String("B".into()));
    }

    #[test]
    fn relation_known_ordered_and_unordered_multivalues_are_canonical() {
        let mut descriptor = relation_fixture();
        descriptor.owned_attributes[0].annotations = vec![Annotation::Card(0, None)];
        descriptor.owned_attributes[0].is_ordered = false;
        let mut first = cartesian_doc("0x101", "0x201");
        first["attributes"]["position"] = serde_json::json!([{"value":"B"},{"value":"A"}]);
        let mut second = first.clone();
        second["attributes"]["position"] = serde_json::json!([{"value":"A"},{"value":"B"}]);
        let rows =
            coalesce_dynamic_relations(&descriptor, &[first.clone(), second.clone()]).unwrap();
        assert_eq!(
            rows[0]
                .attributes
                .iter()
                .map(|(_, v)| v)
                .collect::<Vec<_>>(),
            vec![
                &AttributeValue::String("A".into()),
                &AttributeValue::String("B".into())
            ]
        );

        descriptor.owned_attributes[0].is_ordered = true;
        let row = hydrate_dynamic_relation(&descriptor, &first).unwrap();
        assert_eq!(
            row.attributes
                .iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>(),
            vec![
                &AttributeValue::String("B".into()),
                &AttributeValue::String("A".into())
            ]
        );
        let err = coalesce_dynamic_relations(&descriptor, &[first, second]).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "contradictory relation attribute evidence")
        );
    }

    #[test]
    fn relation_generic_aliases_require_agreement_and_match_indexed_normalization() {
        let descriptor = relation_fixture();
        let mut generic = cartesian_doc("0x101", "0x201");
        generic["role_players"] = serde_json::json!([
            {"role":"employee","iid":"0x101","type_name":"person","attributes":{"name":[{"value":"Alice"}]}},
            {"role":"employer","iid":"0x201","type_name":"company","attributes":{"name":["Acme"]}}
        ]);
        generic["_role_0_iid"] = serde_json::json!("0x101");
        generic["_role_0_type"] = serde_json::json!("person");
        generic["_role_0_attributes"] = serde_json::json!({"name":["Alice"]});
        generic["_role_1_iid"] = serde_json::json!("0x201");
        generic["_role_1_type"] = serde_json::json!("company");
        generic["_role_1_attributes"] = serde_json::json!({"name":["Acme"]});
        let row = hydrate_dynamic_relation(&descriptor, &generic).unwrap();
        assert_eq!(row.role_players.len(), 2);
        assert_eq!(row.role_players[0].role_name, "employee");
        assert_eq!(row.role_players[0].player_iid.as_deref(), Some("0x101"));
        assert_eq!(row.role_players[1].role_name, "employer");
        assert_eq!(row.role_players[1].player_iid.as_deref(), Some("0x201"));
        let mut both = generic.clone();
        both["role_players"][0]["role_name"] = serde_json::json!("employer");
        let err = hydrate_dynamic_relation(&descriptor, &both).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "contradictory player role_name/role evidence")
        );
        let mut malformed = generic.clone();
        malformed["role_players"][0]["role_name"] = serde_json::json!(42);
        let err = hydrate_dynamic_relation(&descriptor, &malformed).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "malformed player role_name/role evidence")
        );
        let mut changed = generic;
        changed["role_players"][0]["attributes"]["name"] = serde_json::json!(["Different"]);
        let err = hydrate_dynamic_relation(&descriptor, &changed).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "contradictory player attribute evidence")
        );
    }

    #[test]
    fn relation_generic_shape_alias_and_player_identity_matrix_is_exact() {
        let descriptor = relation_fixture();
        let base = || {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc.as_object_mut()
                .unwrap()
                .retain(|key, _| !key.starts_with("_role_"));
            doc["role_players"] = serde_json::json!([
                {"role_name":"employee","player_iid":"0x101","player_type_name":"person","attributes":{"name":["Alice"]}},
                {"role_name":"employer","player_iid":"0x201","player_type_name":"company","attributes":{"name":["Acme"]}}
            ]);
            doc
        };
        let mut cases: Vec<(serde_json::Value, &str)> = Vec::new();
        for (field, alias, reason) in [
            ("role_name", "role", "player role is missing"),
            ("player_iid", "iid", "player IID is missing"),
            (
                "player_type_name",
                "type_name",
                "player concrete type is missing",
            ),
        ] {
            let mut doc = base();
            doc["role_players"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            doc["role_players"][0]
                .as_object_mut()
                .unwrap()
                .remove(alias);
            cases.push((doc, reason));
        }
        for (field, alias, reason) in [
            (
                "role_name",
                "role",
                "malformed player role_name/role evidence",
            ),
            (
                "player_iid",
                "iid",
                "malformed player player_iid/iid evidence",
            ),
            (
                "player_type_name",
                "type_name",
                "malformed player player_type_name/type_name evidence",
            ),
        ] {
            let mut doc = base();
            doc["role_players"][0][field] = serde_json::json!(42);
            doc["role_players"][0][alias] = serde_json::json!("employee");
            cases.push((doc, reason));
        }
        for (field, alias, value, reason) in [
            (
                "role_name",
                "role",
                "employer",
                "contradictory player role_name/role evidence",
            ),
            (
                "player_iid",
                "iid",
                "0x999",
                "contradictory player player_iid/iid evidence",
            ),
            (
                "player_type_name",
                "type_name",
                "company",
                "contradictory player player_type_name/type_name evidence",
            ),
        ] {
            let mut doc = base();
            doc["role_players"][0][field] = serde_json::json!("employee");
            doc["role_players"][0][alias] = serde_json::json!(value);
            cases.push((doc, reason));
        }
        for (value, reason) in [
            (serde_json::json!(""), "player IID is not canonical"),
            (serde_json::json!("bad"), "player IID is not canonical"),
        ] {
            let mut doc = base();
            doc["role_players"][0]["player_iid"] = value;
            cases.push((doc, reason));
        }
        let mut blank_type = base();
        blank_type["role_players"][0]["player_type_name"] = serde_json::json!(" ");
        cases.push((blank_type, "player concrete type is blank"));
        for (doc, reason) in cases {
            let err = hydrate_dynamic_relation(&descriptor, &doc).unwrap_err();
            assert!(
                matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == reason)
            );
        }
        let mut wrapped = base();
        wrapped["role_players"][0]["role_name"] = serde_json::json!({"value":"employee"});
        wrapped["role_players"][0]["player_iid"] = serde_json::json!({"value":"0x101"});
        wrapped["role_players"][0]["player_type_name"] = serde_json::json!({"value":"person"});
        assert!(hydrate_dynamic_relation(&descriptor, &wrapped).is_ok());
    }

    #[test]
    fn relation_groups_preserve_first_seen_iid_order_and_optional_roles() {
        let mut descriptor = relation_fixture();
        descriptor.roles[1].cardinality = Some((0, None));
        let mut docs = Vec::new();
        for iid in ["0x20", "0x10", "0x30"] {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc["_iid"] = serde_json::json!(iid);
            doc.as_object_mut().unwrap().retain(|key, _| {
                key != "_role_1_iid" && key != "_role_1_type" && key != "_role_1_attributes"
            });
            docs.push(doc);
        }
        let rows = coalesce_dynamic_relations(&descriptor, &docs).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.iid.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("0x20"), Some("0x10"), Some("0x30")]
        );
        assert!(rows.iter().all(|row| {
            row.role_players
                .iter()
                .all(|player| player.role_name != "employer")
        }));
    }

    #[test]
    fn relation_owned_type_and_attribute_conflicts_are_specific() {
        let descriptor = relation_fixture();
        let mut type_conflict = cartesian_doc("0x101", "0x201");
        type_conflict["_type"] = serde_json::json!("other");
        let err = coalesce_dynamic_relations(
            &descriptor,
            &[cartesian_doc("0x101", "0x201"), type_conflict],
        )
        .unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "contradictory relation type evidence")
        );
        let mut attr_conflict = cartesian_doc("0x101", "0x201");
        attr_conflict["attributes"]["position"] = serde_json::json!([{"value":"Other"}]);
        let err = coalesce_dynamic_relations(
            &descriptor,
            &[cartesian_doc("0x101", "0x201"), attr_conflict],
        )
        .unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "contradictory relation attribute evidence")
        );
    }

    #[test]
    fn relation_ownership_cardinality_matrix_is_authoritative() {
        let mut descriptor = relation_fixture();
        descriptor.owned_attributes[0].annotations = vec![Annotation::Card(0, Some(2))];
        let mut absent = cartesian_doc("0x101", "0x201");
        absent["attributes"] = serde_json::json!({});
        assert!(hydrate_dynamic_relation(&descriptor, &absent).is_ok());
        descriptor.owned_attributes[0].annotations = vec![Annotation::Card(2, Some(2))];
        let err =
            hydrate_dynamic_relation(&descriptor, &cartesian_doc("0x101", "0x201")).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "missing attribute 'position'")
        );
        descriptor.owned_attributes[0].annotations = vec![Annotation::Card(0, Some(1))];
        let mut too_many = cartesian_doc("0x101", "0x201");
        too_many["attributes"]["position"] = serde_json::json!([{"value":"A"},{"value":"B"}]);
        let err = hydrate_dynamic_relation(&descriptor, &too_many).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "relation attribute 'position' violates cardinality")
        );
        let mut optional = descriptor.clone();
        optional.owned_attributes[0].annotations.clear();
        optional.owned_attributes[0].is_optional = true;
        assert!(hydrate_dynamic_relation(&optional, &absent).is_ok());
        let mut required = optional.clone();
        required.owned_attributes[0].is_optional = false;
        let err = hydrate_dynamic_relation(&required, &absent).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "missing attribute 'position'")
        );
        let mut duplicate = cartesian_doc("0x101", "0x201");
        duplicate["attributes"]["position"] = serde_json::json!([{"value":"A"},{"value":"A"}]);
        let err = hydrate_dynamic_relation(&required, &duplicate).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "duplicate scalar relation attribute 'position'")
        );
    }

    #[test]
    fn relation_role_minimum_and_maximum_fail_after_dedup() {
        let mut descriptor = relation_fixture();
        descriptor.roles[0].cardinality = Some((2, Some(2)));
        let docs = vec![
            cartesian_doc("0x101", "0x201"),
            cartesian_doc("0x101", "0x201"),
        ];
        let err = coalesce_dynamic_relations(&descriptor, &docs).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "relation role violates cardinality")
        );
        descriptor.roles[0].cardinality = Some((0, Some(1)));
        let second = cartesian_doc("0x102", "0x201");
        let err =
            coalesce_dynamic_relations(&descriptor, &[cartesian_doc("0x101", "0x201"), second])
                .unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "relation role violates cardinality")
        );
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

    #[test]
    fn relation_public_dto_serialization_is_stable() {
        let row = hydrate_dynamic_relation(&relation_fixture(), &cartesian_doc("0x101", "0x201"))
            .unwrap();
        assert_eq!(
            serde_json::to_value(row).unwrap(),
            serde_json::json!({
                "iid": "0xabc",
                "type_name": "employment",
                "attributes": [["position", {"String": "Engineer"}]],
                "role_players": [
                    {"role_name":"employee", "player_iid":"0x101", "player_type_name":"person", "attributes":[["name", "Alice"]]},
                    {"role_name":"employer", "player_iid":"0x201", "player_type_name":"company", "attributes":[["name", "Acme"]]}
                ]
            })
        );
    }

    #[test]
    fn relation_identity_matrix_has_exact_raw_shape_reasons() {
        let descriptor = relation_fixture();
        let mut cases = Vec::new();
        let mut d = cartesian_doc("0x101", "0x201");
        d.as_object_mut().unwrap().remove("_iid");
        cases.push((d, "relation IID is missing or blank"));
        let mut d = cartesian_doc("0x101", "0x201");
        d["_iid"] = serde_json::json!("");
        cases.push((d, "relation IID is missing or blank"));
        let mut d = cartesian_doc("0x101", "0x201");
        d["_iid"] = serde_json::json!(42);
        cases.push((d, "relation IID is missing or blank"));
        let mut d = cartesian_doc("0x101", "0x201");
        d["_iid"] = serde_json::json!("bad");
        cases.push((d, "relation IID is not canonical"));
        let mut d = cartesian_doc("0x101", "0x201");
        d.as_object_mut().unwrap().remove("_type");
        cases.push((d, "relation concrete type is missing or blank"));
        let mut d = cartesian_doc("0x101", "0x201");
        d["_type"] = serde_json::json!("");
        cases.push((d, "relation concrete type is missing or blank"));
        let mut d = cartesian_doc("0x101", "0x201");
        d["_type"] = serde_json::json!(42);
        cases.push((d, "relation concrete type is missing or blank"));
        for (doc, reason) in cases {
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            assert!(
                matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == reason)
            );
        }
    }

    #[test]
    fn relation_indexed_presence_matrix_has_exact_reasons() {
        let descriptor = relation_fixture();
        for attributes in [serde_json::Value::Null, serde_json::json!({})] {
            let mut optional_descriptor = descriptor.clone();
            optional_descriptor.roles[0].cardinality = Some((0, Some(1)));
            let mut doc = cartesian_doc("0x101", "0x201");
            doc["_role_0_iid"] = serde_json::Value::Null;
            doc["_role_0_type"] = serde_json::Value::Null;
            doc["_role_0_attributes"] = attributes;
            let rows = coalesce_dynamic_relations(&optional_descriptor, &[doc]).unwrap();
            assert_eq!(rows[0].role_players.len(), 1);
            assert_eq!(rows[0].role_players[0].role_name, "employer");
        }
        for key in ["_role_0_iid", "_role_0_type", "_role_0_attributes"] {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc.as_object_mut().unwrap().remove("_role_0_iid");
            doc.as_object_mut().unwrap().remove("_role_0_type");
            doc.as_object_mut().unwrap().remove("_role_0_attributes");
            doc[key] = serde_json::json!("0x101");
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            assert!(
                matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "partial indexed role evidence")
            );
        }
        for (key, reason) in [
            ("_role_0_iid", "indexed player IID/type must be strings"),
            ("_role_0_type", "indexed player IID/type must be strings"),
            (
                "_role_0_attributes",
                "indexed player attributes must be an object",
            ),
        ] {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc[key] = serde_json::json!(42);
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            assert!(
                matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == reason)
            );
        }
        for key in ["_role_x_iid", "_role_0_bad", "_role_2_iid"] {
            let mut doc = cartesian_doc("0x101", "0x201");
            doc[key] = serde_json::json!("0x101");
            let err = coalesce_dynamic_relations(&descriptor, &[doc]).unwrap_err();
            let reason = if key == "_role_2_iid" {
                "unknown indexed relation role"
            } else {
                "malformed indexed relation role"
            };
            assert!(
                matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == reason)
            );
        }
    }

    #[test]
    fn relation_indexed_noncanonical_numeric_keys_are_rejected_exactly() {
        let descriptor = relation_fixture();
        let mut complete = cartesian_doc("0x101", "0x201");
        for key in ["_role_0_iid", "_role_0_type", "_role_0_attributes"] {
            let value = complete.as_object_mut().unwrap().remove(key).unwrap();
            complete[key.replace("_role_0", "_role_00")] = value;
        }
        let err = coalesce_dynamic_relations(&descriptor, &[complete]).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "malformed indexed relation role")
        );

        let mut partial = cartesian_doc("0x101", "0x201");
        let value = partial
            .as_object_mut()
            .unwrap()
            .remove("_role_0_iid")
            .unwrap();
        partial["_role_00_iid"] = value;
        let err = coalesce_dynamic_relations(&descriptor, &[partial]).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "malformed indexed relation role")
        );

        let mut mixed = cartesian_doc("0x101", "0x201");
        mixed["_role_00_iid"] = serde_json::json!("0x102");
        let err = coalesce_dynamic_relations(&descriptor, &[mixed]).unwrap_err();
        assert!(
            matches!(&err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "malformed indexed relation role")
        );

        let canonical = cartesian_doc("0x101", "0x201");
        assert!(hydrate_dynamic_relation(&descriptor, &canonical).is_ok());
    }
}
