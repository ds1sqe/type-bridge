//! Closed source-located YAML authoring for binding-neutral backfills.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_backfill::{
    AttributeBackfillPlan, BackfillPartition, BackfillReverseProgram,
};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::{ManagedDeltaContext, SchemaDocument, YamlMapping, YamlNode};

/// Exact format identifier for the first closed backfill-intent YAML wire.
pub const TYPEBRIDGE_BACKFILL_INTENT_V1: &str = "typebridge.migration-backfill-intent/v1";
/// Maximum bytes accepted for one source backfill intent.
pub const MAX_BACKFILL_INTENT_BYTES: usize = 64 * 1024;

/// Parse and validate one source-located closed copy-attribute intent.
pub fn parse_backfill_intent(
    document: DocumentId,
    bytes: &[u8],
    historical_schema: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<AttributeBackfillPlan, crate::TypeBridgeWorkspaceError> {
    if bytes.len() > MAX_BACKFILL_INTENT_BYTES {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "workspace_backfill_intent_byte_limit",
            "backfill intent exceeds the common source-byte ceiling",
        )
        .into());
    }
    let source = std::str::from_utf8(bytes).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "workspace_backfill_intent_invalid_utf8",
            "backfill intent must be UTF-8 YAML",
        )
    })?;
    let document = SchemaDocument::parse(document, source)?;
    let root = document.root();
    require_keys(root, &["format", "copy-attribute"])?;
    if scalar(required(root, "format")?, "format")? != TYPEBRIDGE_BACKFILL_INTENT_V1 {
        return Err(invalid("format", "workspace_backfill_intent_format").into());
    }
    let operation = mapping(required(root, "copy-attribute")?, "copy-attribute")?;
    require_keys(
        operation,
        &[
            "owner-kind",
            "owner",
            "source",
            "destination",
            "partition-key",
            "batch-rows",
            "reverse",
        ],
    )?;
    let owner_kind = match scalar(required(operation, "owner-kind")?, "owner-kind")? {
        "entity" => TypeKind::Entity,
        "relation" => TypeKind::Relation,
        _ => {
            return Err(invalid("owner-kind", "workspace_backfill_intent_owner_kind").into());
        }
    };
    let owner = TypeId::new(owner_kind, scalar(required(operation, "owner")?, "owner")?)?;
    let source = AttributeId::new(scalar(required(operation, "source")?, "source")?)?;
    let destination =
        AttributeId::new(scalar(required(operation, "destination")?, "destination")?)?;
    let partition_key = AttributeId::new(scalar(
        required(operation, "partition-key")?,
        "partition-key",
    )?)?;
    let batch_rows = canonical_u32(scalar(required(operation, "batch-rows")?, "batch-rows")?)?;
    let reverse = match scalar(required(operation, "reverse")?, "reverse")? {
        "remove-equal-copied-destination" => {
            Some(BackfillReverseProgram::RemoveEqualCopiedDestination)
        }
        "none" => None,
        _ => return Err(invalid("reverse", "workspace_backfill_intent_reverse").into()),
    };
    let semantics = type_bridge_schema::managed_schema_state(historical_schema, context)?
        .managed_semantic_schema()
        .clone();
    Ok(AttributeBackfillPlan::new(
        owner,
        source,
        destination,
        BackfillPartition::new(batch_rows, partition_key)?,
        semantics,
        reverse,
    )?)
}

fn required<'a>(mapping: &'a YamlMapping, key: &str) -> Result<&'a YamlNode, Diagnostic> {
    let mut values = mapping
        .entries()
        .iter()
        .filter(|entry| entry.key().value() == key)
        .map(|entry| entry.value());
    let value = values
        .next()
        .ok_or_else(|| invalid(key, "workspace_backfill_intent_missing_field"))?;
    if values.next().is_some() {
        return Err(invalid(key, "workspace_backfill_intent_duplicate_field"));
    }
    Ok(value)
}

fn require_keys(mapping: &YamlMapping, allowed: &[&str]) -> Result<(), Diagnostic> {
    for entry in mapping.entries() {
        if !allowed.contains(&entry.key().value()) {
            return Err(invalid(
                entry.key().value(),
                "workspace_backfill_intent_unknown_field",
            ));
        }
    }
    Ok(())
}

fn mapping<'a>(node: &'a YamlNode, field: &str) -> Result<&'a YamlMapping, Diagnostic> {
    node.as_mapping()
        .ok_or_else(|| invalid(field, "workspace_backfill_intent_expected_mapping"))
}

fn scalar<'a>(node: &'a YamlNode, field: &str) -> Result<&'a str, Diagnostic> {
    node.as_scalar()
        .map(|scalar| scalar.value())
        .ok_or_else(|| invalid(field, "workspace_backfill_intent_expected_scalar"))
}

fn canonical_u32(value: &str) -> Result<u32, Diagnostic> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid("batch-rows", "workspace_backfill_intent_batch_rows"))?;
    if parsed.to_string() != value {
        return Err(invalid(
            "batch-rows",
            "workspace_backfill_intent_batch_rows",
        ));
    }
    Ok(parsed)
}

fn invalid(field: &str, code: &'static str) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        code,
        "backfill intent violates the closed V1 authoring grammar",
    )
    .with_detail(
        "field",
        type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(field.to_owned()),
    )
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static backfill-intent diagnostic code"),
        message,
    )
}
