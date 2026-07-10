//! Canonical TypeBridge migration-state schema contract.
//!
//! The migration ledger uses the ORM's existing [`SchemaInfo`] IR so schema
//! bootstrap, Rust consumers, and language bindings all project from one
//! structural definition. Labels remain exact-match infrastructure names; no
//! prefix-based reservation is implied.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use type_bridge_orm::schema::SchemaInfo;
use type_bridge_orm::schema::info::{AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry};
use type_bridge_orm::{Annotation, ValueType};

/// Semantic labels used by the TypeDB-backed migration state store.
///
/// Keeping the storage queries and the canonical schema on these constants
/// prevents either surface from acquiring a private copy of the label set.
pub(crate) mod labels {
    /// Entity storing the applied-migration projection.
    pub const APPLIED_ENTITY: &str = "type_bridge_migration";
    /// Entity storing individual migration execution attempts.
    pub const RUN_ENTITY: &str = "type_bridge_migration_run";

    /// Composite applied-migration identifier attribute.
    pub const MIGRATION_ID: &str = "migration_id";
    /// Migration application/package label attribute.
    pub const APP_LABEL: &str = "migration_app_label";
    /// Migration name attribute.
    pub const NAME: &str = "migration_name";
    /// Applied timestamp attribute.
    pub const APPLIED_AT: &str = "migration_applied_at";
    /// Migration checksum attribute.
    pub const CHECKSUM: &str = "migration_checksum";
    /// Execution-attempt identifier attribute.
    pub const RUN_ID: &str = "migration_run_id";
    /// Execution direction attribute.
    pub const DIRECTION: &str = "migration_direction";
    /// Execution status attribute.
    pub const STATUS: &str = "migration_status";
    /// Execution start timestamp attribute.
    pub const STARTED_AT: &str = "migration_started_at";
    /// Execution finish timestamp attribute.
    pub const FINISHED_AT: &str = "migration_finished_at";
    /// Execution error attribute.
    pub const ERROR: &str = "migration_error";
    /// Executor IP address attribute.
    pub const EXECUTOR_IP: &str = "migration_executor_ip";
    /// Executor MAC address attribute.
    pub const EXECUTOR_MAC: &str = "migration_executor_mac";
}

/// Kind of TypeDB schema object tested against the migration-state contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStateSchemaKind {
    /// An entity type label.
    Entity,
    /// A relation type label.
    Relation,
    /// An attribute type label.
    Attribute,
    /// A relation-qualified role label in `relation:role` form.
    Role,
}

static MIGRATION_STATE_SCHEMA: LazyLock<SchemaInfo> = LazyLock::new(build_migration_state_schema);

/// Return TypeBridge's canonical migration-state schema.
///
/// The returned [`SchemaInfo`] is immutable and process-global. It includes
/// every TypeBridge-owned migration entity, relation, attribute, ownership,
/// and role. Consumers should use [`is_migration_state_type`] when they only
/// need membership checks.
pub fn migration_state_schema() -> &'static SchemaInfo {
    &MIGRATION_STATE_SCHEMA
}

/// Return the entity label for the applied-migration projection.
///
/// This semantic accessor keeps compatibility aliases in language bindings
/// tied to the same label constant used by the canonical schema and TypeDB
/// queries.
pub fn applied_migration_entity_label() -> &'static str {
    labels::APPLIED_ENTITY
}

/// Return whether `label` is owned by TypeBridge's migration-state schema.
///
/// Membership is exact and kind-sensitive. Role labels use the qualified
/// `relation:role` form because TypeDB role names are scoped by their relation.
pub fn is_migration_state_type(kind: MigrationStateSchemaKind, label: &str) -> bool {
    let schema = migration_state_schema();
    match kind {
        MigrationStateSchemaKind::Entity => schema.entities.contains_key(label),
        MigrationStateSchemaKind::Relation => schema.relations.contains_key(label),
        MigrationStateSchemaKind::Attribute => schema.attributes.contains_key(label),
        MigrationStateSchemaKind::Role => {
            let Some((relation_label, role_label)) = label.split_once(':') else {
                return false;
            };
            schema
                .relations
                .get(relation_label)
                .is_some_and(|relation| {
                    relation
                        .roles
                        .iter()
                        .any(|role| role.role_name == role_label)
                })
        }
    }
}

fn build_migration_state_schema() -> SchemaInfo {
    use labels::*;

    let attribute_specs = [
        (MIGRATION_ID, ValueType::String),
        (APP_LABEL, ValueType::String),
        (NAME, ValueType::String),
        (APPLIED_AT, ValueType::DateTime),
        (CHECKSUM, ValueType::String),
        (RUN_ID, ValueType::String),
        (DIRECTION, ValueType::String),
        (STATUS, ValueType::String),
        (STARTED_AT, ValueType::DateTime),
        (FINISHED_AT, ValueType::DateTime),
        (ERROR, ValueType::String),
        (EXECUTOR_IP, ValueType::String),
        (EXECUTOR_MAC, ValueType::String),
    ];

    let attributes = attribute_specs
        .into_iter()
        .map(|(label, value_type)| {
            (
                label.to_string(),
                AttributeSchemaEntry::new(label, value_type),
            )
        })
        .collect();

    let entities = [
        (
            APPLIED_ENTITY,
            vec![
                owned_attribute(MIGRATION_ID, ValueType::String, true),
                owned_attribute(APP_LABEL, ValueType::String, false),
                owned_attribute(NAME, ValueType::String, false),
                owned_attribute(APPLIED_AT, ValueType::DateTime, false),
                owned_attribute(CHECKSUM, ValueType::String, false),
            ],
        ),
        (
            RUN_ENTITY,
            vec![
                owned_attribute(RUN_ID, ValueType::String, true),
                owned_attribute(APP_LABEL, ValueType::String, false),
                owned_attribute(NAME, ValueType::String, false),
                owned_attribute(CHECKSUM, ValueType::String, false),
                owned_attribute(DIRECTION, ValueType::String, false),
                owned_attribute(STATUS, ValueType::String, false),
                owned_attribute(STARTED_AT, ValueType::DateTime, false),
                owned_attribute(FINISHED_AT, ValueType::DateTime, false),
                owned_attribute(ERROR, ValueType::String, false),
                owned_attribute(EXECUTOR_IP, ValueType::String, false),
                owned_attribute(EXECUTOR_MAC, ValueType::String, false),
            ],
        ),
    ]
    .into_iter()
    .map(|(label, owned_attributes)| {
        (
            label.to_string(),
            EntitySchemaEntry {
                type_name: label.to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes,
                plays_cardinalities: BTreeMap::new(),
            },
        )
    })
    .collect();

    SchemaInfo {
        entities,
        relations: BTreeMap::new(),
        attributes,
    }
}

fn owned_attribute(label: &str, value_type: ValueType, is_key: bool) -> OwnedAttributeEntry {
    OwnedAttributeEntry {
        attr_name: label.to_string(),
        value_type,
        annotations: if is_key {
            vec![Annotation::Key]
        } else {
            Vec::new()
        },
        is_ordered: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn descriptor_contains_the_complete_current_state_schema() {
        let schema = migration_state_schema();

        assert_eq!(
            schema
                .entities
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([labels::APPLIED_ENTITY, labels::RUN_ENTITY])
        );
        assert_eq!(
            schema
                .attributes
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                labels::MIGRATION_ID,
                labels::APP_LABEL,
                labels::NAME,
                labels::APPLIED_AT,
                labels::CHECKSUM,
                labels::RUN_ID,
                labels::DIRECTION,
                labels::STATUS,
                labels::STARTED_AT,
                labels::FINISHED_AT,
                labels::ERROR,
                labels::EXECUTOR_IP,
                labels::EXECUTOR_MAC,
            ])
        );
        assert!(schema.relations.is_empty());
        assert!(
            schema
                .entities
                .contains_key(applied_migration_entity_label())
        );
    }

    #[test]
    fn descriptor_preserves_value_types_and_key_ownerships() {
        let schema = migration_state_schema();

        assert_eq!(
            schema.attributes[labels::APPLIED_AT].value_type,
            ValueType::DateTime
        );
        assert_eq!(
            schema.attributes[labels::STARTED_AT].value_type,
            ValueType::DateTime
        );
        assert_eq!(
            schema.attributes[labels::FINISHED_AT].value_type,
            ValueType::DateTime
        );

        let applied = &schema.entities[labels::APPLIED_ENTITY];
        let applied_key = applied
            .owned_attributes
            .iter()
            .find(|attribute| attribute.attr_name == labels::MIGRATION_ID)
            .unwrap();
        assert_eq!(applied_key.annotations, vec![Annotation::Key]);

        let run = &schema.entities[labels::RUN_ENTITY];
        let run_key = run
            .owned_attributes
            .iter()
            .find(|attribute| attribute.attr_name == labels::RUN_ID)
            .unwrap();
        assert_eq!(run_key.annotations, vec![Annotation::Key]);
    }

    #[test]
    fn predicate_is_exact_and_kind_sensitive() {
        assert!(is_migration_state_type(
            MigrationStateSchemaKind::Entity,
            labels::APPLIED_ENTITY
        ));
        assert!(is_migration_state_type(
            MigrationStateSchemaKind::Attribute,
            labels::CHECKSUM
        ));
        assert!(!is_migration_state_type(
            MigrationStateSchemaKind::Attribute,
            labels::APPLIED_ENTITY
        ));
        assert!(!is_migration_state_type(
            MigrationStateSchemaKind::Entity,
            "type_bridge_migration_custom"
        ));
        assert!(!is_migration_state_type(
            MigrationStateSchemaKind::Role,
            "unqualified-role"
        ));
    }

    #[test]
    fn canonical_schema_generates_the_existing_typeql_shape() {
        let typeql = migration_state_schema().to_typeql().unwrap();

        assert!(typeql.contains("attribute migration_applied_at, value datetime;"));
        assert!(typeql.contains("attribute migration_started_at, value datetime;"));
        assert!(typeql.contains("entity type_bridge_migration,"));
        assert!(typeql.contains("owns migration_id @key"));
        assert!(typeql.contains("entity type_bridge_migration_run,"));
        assert!(typeql.contains("owns migration_run_id @key"));
    }
}
