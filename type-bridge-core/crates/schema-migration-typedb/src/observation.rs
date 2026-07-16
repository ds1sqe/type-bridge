//! Parsed partitioning of provider schema exports around the control namespace.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, ManagedSchemaState, SourcedSchemaFact,
};
use type_bridge_migration::migration_state_schema;
use type_bridge_schema::{DeltaError, ManagedDeltaContext, managed_schema_state};
use type_bridge_schema_compat::{
    SchemaReference, TypeqlDeclaredSchema, typeql_to_declared,
    typeql_to_declared_with_references,
};

use crate::control_schema::{
    MANAGED_FENCE_SCHEMA_TYPEQL, TYPEBRIDGE_INTERNAL_PREFIX,
};
use crate::is_typebridge_internal_label;

/// The exact-match label set of the frozen legacy (v1) migration ledger.
///
/// A v1 database keeps its applied ledger as ordinary entities inside the
/// managed database, so observation must recognize those frozen labels as
/// TypeBridge control state rather than user content. The set is exact —
/// legacy reservation was never prefix-based.
static LEGACY_CONTROL_LABELS: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    let schema = migration_state_schema();
    schema
        .entities
        .keys()
        .chain(schema.relations.keys())
        .chain(schema.attributes.keys())
        .map(String::as_str)
        .collect()
});

/// One parsed TypeDB export partitioned into managed-user and internal facts.
#[derive(Clone, Debug)]
pub struct PartitionedDeclaredSchema {
    full: DeclaredSchema,
    user: DeclaredSchema,
    internal: DeclaredSchema,
    legacy_control: DeclaredSchema,
}

impl PartitionedDeclaredSchema {
    /// Return the complete parsed provider export.
    #[must_use]
    pub const fn full(&self) -> &DeclaredSchema {
        &self.full
    }

    /// Return all non-control facts used for managed-state observation.
    #[must_use]
    pub const fn user(&self) -> &DeclaredSchema {
        &self.user
    }

    /// Return the reserved control-schema facts.
    #[must_use]
    pub const fn internal(&self) -> &DeclaredSchema {
        &self.internal
    }

    /// Return the frozen legacy (v1) migration-ledger facts.
    #[must_use]
    pub const fn legacy_control(&self) -> &DeclaredSchema {
        &self.legacy_control
    }
}

/// Parse one full TypeDB export and partition it without editing raw TypeQL.
pub fn partition_typeql_export(
    document: DocumentId,
    source: &str,
) -> Result<PartitionedDeclaredSchema, Diagnostic> {
    let full = typeql_to_declared(document, source).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_export_invalid",
            "TypeDB schema export cannot be normalized into V2 facts",
        )
    })?;
    partition_declared_schema(full)
}

pub(crate) fn partition_declared_schema(
    full: DeclaredSchema,
) -> Result<PartitionedDeclaredSchema, Diagnostic> {
    let mut user = Vec::new();
    let mut internal = Vec::new();
    let mut legacy_control = Vec::new();
    for fact in full.facts() {
        let id = fact.id();
        let source = full.source(&id).cloned().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_export_missing_provenance",
                "normalized TypeDB export fact has no source provenance",
            )
        })?;
        let id_value = serde_json::to_value(&id).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_fact_identity_encode_failed",
                "schema fact identity cannot be inspected for reserved labels",
            )
        })?;
        let sourced = SourcedSchemaFact::new(fact.clone(), source);
        if value_mentions_reserved_label(&id_value) {
            internal.push(sourced);
        } else if value_mentions_legacy_control_label(&id_value) {
            legacy_control.push(sourced);
        } else {
            user.push(sourced);
        }
    }

    let user = DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), user).map_err(
        |_| {
            failure(
                DiagnosticCategory::Integrity,
                "reserved_schema_cross_reference",
                "user schema facts reference the reserved TypeDB control namespace",
            )
        },
    )?;
    let internal = DeclaredSchema::from_facts(
        full.format(),
        CapabilitySet::new(),
        internal,
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "reserved_schema_cross_reference",
            "reserved TypeDB control facts reference user schema declarations",
        )
    })?;
    let legacy_control = DeclaredSchema::from_facts(
        full.format(),
        CapabilitySet::new(),
        legacy_control,
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "reserved_schema_cross_reference",
            "legacy migration-ledger facts reference user schema declarations",
        )
    })?;

    Ok(PartitionedDeclaredSchema {
        full,
        user,
        internal,
        legacy_control,
    })
}

/// Observe the exact live managed schema state from one provider export.
///
/// The candidates supply scope, semantic-profile, format, and
/// required-capability context; their fingerprint claims never enter the
/// observation. The export is parsed once, its function bodies must be
/// statically free of reserved and dynamic type references, its reserved
/// partition must equal the frozen fence-mirror contract exactly, every
/// non-internal fact is treated as managed, and the state rebuilt from live
/// facts must equal exactly one distinct candidate.
pub fn observe_managed_state_from_export(
    document: DocumentId,
    export: &str,
    available_capabilities: &CapabilitySet,
    source_candidate: &ManagedSchemaState,
    target_candidate: &ManagedSchemaState,
) -> Result<ManagedSchemaState, Diagnostic> {
    let parsed =
        typeql_to_declared_with_references(document, export).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_export_invalid",
                "TypeDB schema export cannot be normalized into V2 facts",
            )
        })?;
    reject_reserved_function_references(&parsed)?;
    let partitioned = partition_declared_schema(parsed.into_declared())?;
    verify_fence_mirror_partition(partitioned.internal())?;
    verify_legacy_control_partition(partitioned.legacy_control())?;

    let mut matched: Vec<ManagedSchemaState> = Vec::new();
    for candidate in [source_candidate, target_candidate] {
        let Ok(rebuilt) =
            rebuild_candidate_state(&partitioned, available_capabilities, candidate)
        else {
            continue;
        };
        if &rebuilt == candidate && !matched.contains(&rebuilt) {
            matched.push(rebuilt);
        }
    }
    match matched.len() {
        1 => Ok(matched.remove(0)),
        0 => Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_observation_no_candidate_match",
            "live managed schema state equals neither supplied candidate",
        )
        .with_detail(
            "scope",
            source_candidate.scope().id().as_str().to_owned(),
        )),
        _ => Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_observation_ambiguous",
            "live managed schema state matches contradictory candidates",
        )),
    }
}

fn reject_reserved_function_references(
    parsed: &TypeqlDeclaredSchema,
) -> Result<(), Diagnostic> {
    for (function, references) in parsed.function_body_references() {
        if references.has_dynamic_type_reference() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_dynamic_function_reference",
                "function body supplies a type position dynamically and cannot be \
                 proven free of the reserved control namespace",
            )
            .with_detail("function", function.label().as_str().to_owned()));
        }
        for reference in references.references() {
            let control_label = |label: &str| {
                is_typebridge_internal_label(label)
                    || LEGACY_CONTROL_LABELS.contains(label)
            };
            let reserved = match reference {
                SchemaReference::Label(label) => control_label(label.as_str()),
                SchemaReference::Scoped { scope, name } => {
                    control_label(scope.as_str()) || control_label(name.as_str())
                }
                SchemaReference::Function(id) => control_label(id.label().as_str()),
            };
            if reserved {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "reserved_schema_cross_reference",
                    "function body references the reserved TypeDB control namespace",
                )
                .with_detail("function", function.label().as_str().to_owned()));
            }
        }
    }
    Ok(())
}

fn verify_fence_mirror_partition(internal: &DeclaredSchema) -> Result<(), Diagnostic> {
    let expected_document =
        DocumentId::new("typebridge-managed-fence-schema.typeql")?;
    let expected = typeql_to_declared(expected_document, MANAGED_FENCE_SCHEMA_TYPEQL)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_frozen_schema_invalid",
                "frozen TypeDB fence-mirror schema cannot be normalized",
            )
        })?;
    if internal.declared_identity_fingerprint()
        != expected.declared_identity_fingerprint()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_control_schema_mismatch",
            "reserved fence-mirror schema differs from the frozen contract",
        ));
    }
    Ok(())
}

/// Require the legacy partition to be absent or the exact frozen v1 ledger.
///
/// A v1 database carries the complete frozen ledger schema; a database that
/// never ran v1 migrations carries none of it. Anything in between — a
/// partial install, or a user schema that happens to reuse a frozen legacy
/// label — is indistinguishable from corruption and fails closed instead of
/// silently reclassifying user content as control state.
fn verify_legacy_control_partition(
    legacy_control: &DeclaredSchema,
) -> Result<(), Diagnostic> {
    if legacy_control.facts().next().is_none() {
        return Ok(());
    }
    let frozen = migration_state_schema().to_typeql().map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_frozen_schema_invalid",
            "frozen legacy migration-ledger schema cannot be rendered",
        )
    })?;
    let expected_document =
        DocumentId::new("typebridge-legacy-ledger-schema.typeql")?;
    let expected = typeql_to_declared(expected_document, &frozen).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_frozen_schema_invalid",
            "frozen legacy migration-ledger schema cannot be normalized",
        )
    })?;
    if legacy_control.declared_identity_fingerprint()
        != expected.declared_identity_fingerprint()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_legacy_ledger_mismatch",
            "legacy migration-ledger facts differ from the frozen v1 contract",
        ));
    }
    Ok(())
}

fn rebuild_candidate_state(
    partitioned: &PartitionedDeclaredSchema,
    available_capabilities: &CapabilitySet,
    candidate: &ManagedSchemaState,
) -> Result<ManagedSchemaState, Diagnostic> {
    let semantic_profile = candidate
        .managed_semantic_schema()
        .as_fingerprint()
        .semantic_profile()
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_candidate_profile_missing",
                "candidate managed state carries no semantic-profile identity",
            )
        })?
        .clone();
    let context = ManagedDeltaContext::new(
        candidate.scope().id().clone(),
        semantic_profile,
        available_capabilities.clone(),
    );
    let user = partitioned.user();
    let facts = user
        .facts()
        .map(|fact| {
            let id = fact.id();
            let source = user.source(&id).cloned().ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_export_missing_provenance",
                    "normalized TypeDB export fact has no source provenance",
                )
            })?;
            Ok(SourcedSchemaFact::new(fact.clone(), source))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let declared = DeclaredSchema::from_facts(
        candidate.format(),
        candidate.required_capabilities().clone(),
        facts,
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_observation_rebuild_failed",
            "live managed facts cannot form a declared schema under the candidate context",
        )
    })?;
    managed_schema_state(&declared, &context).map_err(|error| match error {
        DeltaError::Contract(diagnostic) => diagnostic,
        DeltaError::Schema(diagnostics) => diagnostics
            .iter()
            .next()
            .map(|entry| entry.diagnostic().clone())
            .unwrap_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_observation_rebuild_failed",
                    "live managed schema does not resolve under the candidate context",
                )
            }),
    })
}

fn value_mentions_reserved_label(value: &Value) -> bool {
    match value {
        Value::String(value) => value.starts_with(TYPEBRIDGE_INTERNAL_PREFIX),
        Value::Array(values) => values.iter().any(value_mentions_reserved_label),
        Value::Object(values) => values.values().any(value_mentions_reserved_label),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn value_mentions_legacy_control_label(value: &Value) -> bool {
    match value {
        Value::String(value) => LEGACY_CONTROL_LABELS.contains(value.as_str()),
        Value::Array(values) => values.iter().any(value_mentions_legacy_control_label),
        Value::Object(values) => {
            values.values().any(value_mentions_legacy_control_label)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_schema_migration::typedb_3_12_1_profile;

    use super::*;

    fn observation_document() -> DocumentId {
        DocumentId::new("observer-live-export.typeql").expect("document id")
    }

    fn available_capabilities() -> CapabilitySet {
        typedb_3_12_1_profile().required_capabilities.clone()
    }

    fn candidate_state(user_typeql: &str) -> ManagedSchemaState {
        let document = DocumentId::new("observer-candidate.typeql").expect("document id");
        let declared = typeql_to_declared(document, user_typeql).expect("candidate schema");
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("observer-test-scope").expect("scope"),
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            available_capabilities(),
        );
        managed_schema_state(&declared, &context).expect("candidate state")
    }

    fn export_with_user(user_definables: &str) -> String {
        format!("{MANAGED_FENCE_SCHEMA_TYPEQL}{user_definables}")
    }

    #[test]
    fn observes_the_exact_unique_matching_candidate() {
        let export = export_with_user("entity person;\nentity company;\n");
        let source = candidate_state("define\nentity person;\n");
        let target = candidate_state("define\nentity person;\nentity company;\n");
        let observed = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &source,
            &target,
        )
        .expect("live state matches the target candidate");
        assert_eq!(observed, target);
        assert_ne!(observed, source);

        let equal = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &target,
            &target,
        )
        .expect("equal candidates deduplicate to one unique match");
        assert_eq!(equal, target);
    }

    #[test]
    fn rejects_live_state_matching_no_candidate() {
        let export = export_with_user("entity person;\nentity company;\n");
        let stale = candidate_state("define\nentity person;\n");
        let error = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &stale,
            &stale,
        )
        .expect_err("foreign live state must not observe");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_observation_no_candidate_match",
        );
    }

    #[test]
    fn rejects_missing_fence_mirror_partition() {
        let export = "define\nentity person;\n";
        let candidate = candidate_state("define\nentity person;\n");
        let error = observe_managed_state_from_export(
            observation_document(),
            export,
            &available_capabilities(),
            &candidate,
            &candidate,
        )
        .expect_err("export without the fence mirror must not observe");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_control_schema_mismatch",
        );
    }

    #[test]
    fn legacy_ledger_facts_stay_out_of_the_managed_observation() {
        let legacy = migration_state_schema().to_typeql().expect("legacy typeql");
        let legacy_definables = legacy
            .trim_start()
            .strip_prefix("define")
            .unwrap_or(&legacy)
            .to_owned();
        let export = export_with_user(&format!("{legacy_definables}\nentity person;\n"));
        let candidate = candidate_state("define\nentity person;\n");
        let observed = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &candidate,
            &candidate,
        )
        .expect("the frozen legacy ledger is control state, not managed content");
        assert_eq!(observed, candidate);
    }

    #[test]
    fn partial_legacy_ledger_facts_fail_closed() {
        let export = export_with_user(
            "attribute migration_id, value string;\nentity person;\n",
        );
        let candidate = candidate_state("define\nentity person;\n");
        let error = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &candidate,
            &candidate,
        )
        .expect_err("a partial legacy ledger is indistinguishable from corruption");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_legacy_ledger_mismatch",
        );
    }

    #[test]
    fn rejects_dynamic_function_type_references() {
        let export = export_with_user(
            "entity person;\nfun people($candidate: person) -> { person }: \
             match $candidate isa $kind; return { $candidate };\n",
        );
        let candidate = candidate_state("define\nentity person;\n");
        let error = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &candidate,
            &candidate,
        )
        .expect_err("dynamic type references cannot be proven reserved-free");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_dynamic_function_reference",
        );
    }

    #[test]
    fn rejects_reserved_function_body_references() {
        let export = export_with_user(
            "entity person;\nfun spies($candidate: person) -> { person }: \
             match $candidate isa person; \
             $control isa typebridge-internal-v2-migration-control; \
             return { $candidate };\n",
        );
        let candidate = candidate_state("define\nentity person;\n");
        let error = observe_managed_state_from_export(
            observation_document(),
            &export,
            &available_capabilities(),
            &candidate,
            &candidate,
        )
        .expect_err("reserved control references in function bodies must not observe");
        assert_eq!(error.code().as_str(), "reserved_schema_cross_reference");
    }
}
