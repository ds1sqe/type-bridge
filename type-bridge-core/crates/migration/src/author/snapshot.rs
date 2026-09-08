//! Render the immutable `snapshots/vNNNN/` artifact set from a target schema.
//!
//! Mirrors the historical Python snapshot pipeline byte-for-byte:
//! `SchemaInfo -> TypeQL -> bindgen -> binding modules`, the rewritten
//! snapshot `__init__.py`, the bundled `schema.tql`, and the
//! `snapshot.json` hash manifest. Generated schema text contains no comment
//! annotations, so bindgen runs with default Python render metadata exactly
//! like the live flow does for snapshots.

use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};
use type_bridge_core_lib::_bindgen::{BindgenOptions, BindgenPlan, TargetLanguage};
use type_bridge_core_lib::_schema::TypeSchema;
use type_bridge_orm::_schema::info::SchemaInfo;

use crate::error::MigrationError;

/// Inputs for rendering one snapshot version.
pub struct SnapshotRenderRequest<'a> {
    /// The schema this snapshot captures (the authoring target).
    pub target: &'a SchemaInfo,
    /// Snapshot version directory name (e.g. `v0003`).
    pub version: &'a str,
    /// Migration stem this snapshot belongs to (e.g. `0003_add_assignment`).
    pub source_migration: &'a str,
    /// `type_bridge` package version recorded in the manifest.
    pub type_bridge_version: &'a str,
    /// `type_bridge_core` version recorded in the manifest.
    pub type_bridge_core_version: &'a str,
}

/// One rendered snapshot: file contents keyed by path relative to the
/// migrations directory (e.g. `snapshots/v0003/entities.py`).
pub struct RenderedSnapshot {
    /// Rendered files in deterministic order.
    pub files: Vec<(String, Vec<u8>)>,
    /// The canonical schema text the snapshot was rendered from.
    pub schema_text: String,
}

#[derive(Serialize)]
struct SnapshotManifest<'a> {
    version: &'a str,
    source_migration: &'a str,
    schema_hash: String,
    file_hashes: BTreeMap<String, String>,
    type_bridge_version: &'a str,
    type_bridge_core_version: &'a str,
}

/// Render the complete snapshot file set in memory.
///
/// # Errors
///
/// [`MigrationError::SchemaGeneration`] when the target schema cannot be
/// rendered to TypeQL or parsed back for bindgen.
pub fn render_snapshot(request: &SnapshotRenderRequest<'_>) -> crate::Result<RenderedSnapshot> {
    let schema_text = request
        .target
        .to_typeql()
        .map_err(|e| MigrationError::SchemaGeneration {
            message: e.to_string(),
        })?;

    let is_empty_schema = request.target.entities.is_empty()
        && request.target.relations.is_empty()
        && request.target.attributes.is_empty();
    let type_schema = if is_empty_schema {
        // A complete teardown renders a bare `define` block that the TypeQL
        // parser rejects; bindgen an explicitly empty schema instead.
        TypeSchema {
            entities: BTreeMap::new(),
            relations: BTreeMap::new(),
            attributes: BTreeMap::new(),
            functions: BTreeMap::new(),
            structs: BTreeMap::new(),
        }
    } else {
        TypeSchema::from_typeql(&schema_text).map_err(|e| MigrationError::SchemaGeneration {
            message: format!("snapshot schema failed to parse: {e}"),
        })?
    };

    let options = BindgenOptions {
        schema_text: Some(schema_text.clone()),
        ..BindgenOptions::default()
    };
    // Match the live generator byte-for-byte: every snapshot attaches the
    // declared descriptor set exactly like `generate_models` does. A
    // complete teardown attaches the canonical empty closed world, so the
    // snapshot file contract does not depend on schema cardinality.
    let package = if is_empty_schema {
        let mut package =
            BindgenPlan::from_schema(&type_schema).render(TargetLanguage::Python, &options);
        let descriptors = type_bridge_schema_compat::empty_generated_declared_descriptors_json()
            .map_err(|message| MigrationError::SchemaGeneration { message })?;
        type_bridge_schema_compat::attach_declared_descriptors(
            &mut package,
            descriptors,
            TargetLanguage::Python,
        )
        .map_err(|message| MigrationError::SchemaGeneration { message })?;
        package
    } else {
        type_bridge_schema_compat::generate_package_with_declared_descriptors(
            &schema_text,
            TargetLanguage::Python,
            &options,
        )
        .map_err(|message| MigrationError::SchemaGeneration { message })?
    };

    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    for file in package.files {
        modules.insert(file.path, file.contents);
    }
    modules.insert("schema.tql".to_string(), schema_text.clone());
    modules.insert("__init__.py".to_string(), render_snapshot_init(&modules));

    let file_hashes: BTreeMap<String, String> = modules
        .iter()
        .map(|(name, contents)| (name.clone(), sha256_hex(contents.as_bytes())))
        .collect();
    let manifest = SnapshotManifest {
        version: request.version,
        source_migration: request.source_migration,
        schema_hash: sha256_hex(schema_text.as_bytes()),
        file_hashes,
        type_bridge_version: request.type_bridge_version,
        type_bridge_core_version: request.type_bridge_core_version,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)? + "\n";
    modules.insert("snapshot.json".to_string(), manifest_json);

    let prefix = format!("snapshots/{}", request.version);
    let mut files: Vec<(String, Vec<u8>)> = vec![(
        "snapshots/__init__.py".to_string(),
        b"# TypeBridge migration snapshots package\n".to_vec(),
    )];
    files.extend(
        modules
            .into_iter()
            .map(|(name, contents)| (format!("{prefix}/{name}"), contents.into_bytes())),
    );

    Ok(RenderedSnapshot { files, schema_text })
}

/// Rewrite the snapshot package `__init__.py`, mirroring the historical
/// Python `_rewrite_snapshot_init` template exactly.
fn render_snapshot_init(modules: &BTreeMap<String, String>) -> String {
    let attributes = class_names(modules.get("attributes.py"));
    let entities = class_names(modules.get("entities.py"));
    let relations = class_names(modules.get("relations.py"));

    let mut lines: Vec<String> = vec![
        "\"\"\"TypeBridge migration snapshot bindings generated from a TypeDB schema.".to_string(),
        String::new(),
        "AUTO-GENERATED FILE - DO NOT EDIT MANUALLY".to_string(),
        "\"\"\"".to_string(),
        String::new(),
        "from __future__ import annotations".to_string(),
        String::new(),
        "from importlib import resources".to_string(),
        String::new(),
        "from . import attributes, entities, registry, relations".to_string(),
    ];
    for (module, names) in [
        ("attributes", &attributes),
        ("entities", &entities),
        ("relations", &relations),
    ] {
        if let Some(block) = render_class_import(module, names) {
            lines.push(String::new());
            lines.push(block);
        }
    }
    lines.extend([
        String::new(),
        "SCHEMA_VERSION = \"1.0.0\"".to_string(),
        String::new(),
        String::new(),
        "def schema_text() -> str:".to_string(),
        "    \"\"\"Return the canonical TypeDB schema text bundled with the package.\"\"\""
            .to_string(),
        "    return (".to_string(),
        "        resources.files(__package__)".to_string(),
        "        .joinpath(\"schema.tql\")".to_string(),
        "        .read_text(encoding=\"utf-8\")".to_string(),
        "    )".to_string(),
        String::new(),
        render_class_list("ATTRIBUTES", &attributes),
        String::new(),
        render_class_list("ENTITIES", &entities),
        String::new(),
        render_class_list("RELATIONS", &relations),
        String::new(),
        render_all(&attributes, &entities, &relations),
    ]);
    lines.join("\n") + "\n"
}

/// Extract top-level class names from a generated module, in source order.
///
/// The generated modules are bindgen's own deterministic output, so a plain
/// line scan is equivalent to the AST walk the Python side performs.
fn class_names(contents: Option<&String>) -> Vec<String> {
    let Some(contents) = contents else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("class ")?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

fn render_class_import(module: &str, names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let joined = names.join(", ");
    let single_line = format!("from .{module} import {joined}");
    if single_line.len() <= 88 {
        return Some(single_line);
    }
    let mut lines = vec![format!("from .{module} import (")];
    lines.extend(names.iter().map(|name| format!("    {name},")));
    lines.push(")".to_string());
    Some(lines.join("\n"))
}

fn render_class_list(name: &str, class_names: &[String]) -> String {
    let mut lines = vec![format!("{name} = [")];
    lines.extend(
        class_names
            .iter()
            .map(|class_name| format!("    {class_name},")),
    );
    lines.push("]".to_string());
    lines.join("\n")
}

fn render_all(attributes: &[String], entities: &[String], relations: &[String]) -> String {
    let mut lines = vec!["__all__ = [".to_string()];
    for name in attributes
        .iter()
        .chain(entities)
        .chain(relations)
        .map(String::as_str)
        .chain([
            "ATTRIBUTES",
            "ENTITIES",
            "RELATIONS",
            "SCHEMA_VERSION",
            "attributes",
            "entities",
            "registry",
            "relations",
            "schema_text",
        ])
    {
        lines.push(format!("    \"{name}\","));
    }
    lines.push("]".to_string());
    lines.join("\n")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
