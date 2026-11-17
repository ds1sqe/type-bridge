//! Confined YAML authoring of atomic data-step programs.
use crate::TypeBridgeWorkspaceError;
use std::io::Read;
use std::path::Path;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_contract::migration_data::{DataProgram, DataStep};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::{SchemaDocument, YamlMapping, YamlNode};
use type_bridge_schema_migration::{MigrationDirectory, validate_portable_direct_child};

fn invalid(code: &'static str, field: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static data-intent code"),
        "data-step intent violates the closed authoring grammar",
    )
    .with_detail("field", field.to_owned())
}
fn keys(map: &YamlMapping, allowed: &[&str]) -> Result<(), Diagnostic> {
    let mut seen = std::collections::BTreeSet::new();
    for entry in map.entries() {
        let key = entry.key().value();
        if !allowed.contains(&key) {
            return Err(invalid("workspace_data_intent_unknown_field", key));
        }
        if !seen.insert(key) {
            return Err(invalid("workspace_data_intent_duplicate_field", key));
        }
    }
    Ok(())
}
fn get<'a>(map: &'a YamlMapping, key: &str) -> Option<&'a YamlNode> {
    map.entries()
        .iter()
        .find(|entry| entry.key().value() == key)
        .map(|entry| entry.value())
}
fn required<'a>(map: &'a YamlMapping, key: &str) -> Result<&'a YamlNode, Diagnostic> {
    get(map, key).ok_or_else(|| invalid("workspace_data_intent_missing_field", key))
}
fn text<'a>(node: &'a YamlNode, key: &str) -> Result<&'a str, Diagnostic> {
    node.as_scalar()
        .map(|value| value.value())
        .ok_or_else(|| invalid("workspace_data_intent_expected_scalar", key))
}

pub(crate) fn read_file(
    directory: &MigrationDirectory,
    path: &Path,
) -> Result<Vec<u8>, TypeBridgeWorkspaceError> {
    validate_portable_direct_child(path.as_os_str())
        .map_err(|_| invalid("workspace_data_intent_path_not_confined", "file"))?;
    let file = directory
        .open_regular_readonly(path.as_os_str())
        .map_err(|_| invalid("workspace_data_intent_file_unreadable", "file"))?;
    let max = type_bridge_contract::limits::MAX_CANONICAL_BYTES;
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("workspace_data_intent_file_unreadable", "file"))?;
    if bytes.len() > max {
        return Err(invalid("workspace_data_intent_byte_limit", "file").into());
    }
    Ok(bytes)
}
fn program(
    node: &YamlNode,
    directory: &MigrationDirectory,
    schema: &DeclaredSchema,
) -> Result<DataProgram, TypeBridgeWorkspaceError> {
    let map = node
        .as_mapping()
        .ok_or_else(|| invalid("workspace_data_intent_expected_mapping", "program"))?;
    keys(map, &["user", "typeql", "plan-file"])?;
    if map.entries().len() != 1 {
        return Err(invalid("workspace_data_intent_program_arity", "program").into());
    }
    let entry = &map.entries()[0];
    let key = entry.key().value();
    let value = text(entry.value(), key)?;
    Ok(match key {
        "typeql" => DataProgram::Typeql {
            text: value.to_owned(),
        },
        "plan-file" => {
            // Intents are direct children; their retained directory is the base.
            let bytes = read_file(directory, Path::new(value))?;
            DataProgram::Plan {
                plan: Box::new(type_bridge_contract::query_plan::decode_query_plan(
                    &bytes, schema,
                )?),
            }
        }
        "user" => DataProgram::User {
            step_id: MigrationStepId::new(value)?,
        },
        _ => unreachable!("closed program keys were validated"),
    })
}

pub(crate) fn parse(
    document: DocumentId,
    bytes: &[u8],
    directory: &MigrationDirectory,
    schema: &DeclaredSchema,
) -> Result<DataStep, TypeBridgeWorkspaceError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| invalid("workspace_data_intent_utf8", "document"))?;
    let doc = SchemaDocument::parse(document, source)?;
    let map = doc.root();
    keys(map, &["format", "id", "forward", "reverse"])?;
    if text(required(map, "format")?, "format")? != "typebridge.migration-data-step-intent/v1" {
        return Err(invalid("workspace_data_intent_format", "format").into());
    }
    Ok(DataStep::new(
        MigrationStepId::new(text(required(map, "id")?, "id")?)?,
        program(required(map, "forward")?, directory, schema)?,
        get(map, "reverse")
            .map(|node| program(node, directory, schema))
            .transpose()?,
    )?)
}
