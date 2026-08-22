use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::TypeKind;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::ManagedDeltaContext;
use type_bridge_workspace::{MAX_BACKFILL_INTENT_BYTES, parse_backfill_intent};

const VALID: &str = r#"format: typebridge.migration-backfill-intent/v1
copy-attribute:
  owner-kind: entity
  owner: person
  source: legacy-name
  destination: display-name
  partition-key: person-id
  batch-rows: 128
  reverse: remove-equal-copied-destination
"#;

fn schema() -> DeclaredSchema {
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), std::iter::empty()).unwrap()
}

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("intent-test").unwrap(),
        SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        CapabilitySet::new(),
    )
}

fn parse(
    source: &[u8],
) -> Result<
    type_bridge_contract::migration_backfill::AttributeBackfillPlan,
    type_bridge_workspace::TypeBridgeWorkspaceError,
> {
    parse_backfill_intent(
        DocumentId::new("copy-name.backfill.yaml").unwrap(),
        source,
        &schema(),
        &context(),
    )
}

fn error_code(error: &type_bridge_workspace::TypeBridgeWorkspaceError) -> &str {
    if let Some(contract) = error.contract() {
        contract.code().as_str()
    } else {
        error
            .schema()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str()
    }
}

#[test]
fn parses_the_closed_source_located_copy_attribute_intent() {
    let plan = parse(VALID.as_bytes()).unwrap();
    assert_eq!(plan.owner().kind(), TypeKind::Entity);
    assert_eq!(plan.owner().label().as_str(), "person");
    assert_eq!(plan.source().label().as_str(), "legacy-name");
    assert_eq!(plan.destination().label().as_str(), "display-name");
    assert_eq!(
        plan.partition().stable_attribute().label().as_str(),
        "person-id"
    );
    assert_eq!(plan.partition().batch_rows(), 128);
    assert!(plan.reverse().is_some());
}

#[test]
fn rejects_unknown_duplicate_noncanonical_and_oversized_input() {
    for (source, code) in [
        (
            VALID.replace("copy-attribute:", "unknown: true\ncopy-attribute:"),
            "workspace_backfill_intent_unknown_field",
        ),
        (
            VALID.replace("  owner: person", "  owner: person\n  owner: company"),
            "duplicate_yaml_key",
        ),
        (
            VALID.replace("batch-rows: 128", "batch-rows: 0128"),
            "workspace_backfill_intent_batch_rows",
        ),
    ] {
        assert_eq!(error_code(&parse(source.as_bytes()).unwrap_err()), code);
    }

    let oversized = vec![b' '; MAX_BACKFILL_INTENT_BYTES + 1];
    assert_eq!(
        error_code(&parse(&oversized).unwrap_err()),
        "workspace_backfill_intent_byte_limit"
    );
}

#[test]
fn rejects_open_ended_or_executable_escape_hatches() {
    for forbidden in [
        "  query: match $x isa person;\n",
        "  callback: module.function\n",
        "  command: migrate.sh\n",
        "  endpoint: https://example.invalid\n",
    ] {
        let source = VALID.replace("  reverse:", &format!("{forbidden}  reverse:"));
        assert_eq!(
            parse(source.as_bytes())
                .unwrap_err()
                .contract()
                .unwrap()
                .code()
                .as_str(),
            "workspace_backfill_intent_unknown_field"
        );
    }
}
