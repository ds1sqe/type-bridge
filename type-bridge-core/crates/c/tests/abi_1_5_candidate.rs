#![cfg(feature = "abi-1-5")]

use std::collections::BTreeSet;

const ABI_1_5_CANDIDATE_ADDITIONS: [&str; 86] = [
    "type_bridge_database_administration_open",
    "type_bridge_database_administration_exists",
    "type_bridge_database_administration_exists_with_options",
    "type_bridge_database_administration_create",
    "type_bridge_database_administration_create_with_options",
    "type_bridge_database_administration_inspect",
    "type_bridge_database_administration_inspect_with_options",
    "type_bridge_database_administration_plan_delete",
    "type_bridge_database_administration_plan_delete_with_options",
    "type_bridge_database_deletion_plan_inspected_state",
    "type_bridge_database_deletion_plan_execute",
    "type_bridge_database_deletion_plan_execute_with_options",
    "type_bridge_database_deletion_plan_close",
    "type_bridge_database_administration_close",
    "type_bridge_migration_catalog_open",
    "type_bridge_migration_catalog_fingerprint",
    "type_bridge_migration_catalog_count",
    "type_bridge_migration_catalog_entry_at",
    "type_bridge_migration_catalog_head_count",
    "type_bridge_migration_catalog_head_at",
    "type_bridge_migration_catalog_close",
    "type_bridge_migration_history_entry_identity",
    "type_bridge_migration_history_entry_parent_count",
    "type_bridge_migration_history_entry_parent_at",
    "type_bridge_migration_history_entry_manifest_digest",
    "type_bridge_migration_history_entry_step_count",
    "type_bridge_migration_history_entry_safety",
    "type_bridge_migration_history_entry_reversible",
    "type_bridge_migration_history_entry_close",
    "type_bridge_migration_identity_app_label",
    "type_bridge_migration_identity_name",
    "type_bridge_migration_identity_close",
    "type_bridge_migration_catalog_preview_apply",
    "type_bridge_migration_catalog_preview_rollback",
    "type_bridge_migration_plan_direction",
    "type_bridge_migration_plan_execution_authorized",
    "type_bridge_migration_plan_count",
    "type_bridge_migration_plan_entry_at",
    "type_bridge_migration_plan_approval_builder",
    "type_bridge_migration_plan_authorize",
    "type_bridge_migration_plan_execute",
    "type_bridge_migration_cancellation_new",
    "type_bridge_migration_cancellation_cancel",
    "type_bridge_migration_cancellation_is_cancelled",
    "type_bridge_migration_cancellation_close",
    "type_bridge_migration_plan_execute_with_options",
    "type_bridge_migration_plan_close",
    "type_bridge_migration_plan_entry_identity",
    "type_bridge_migration_plan_entry_safety",
    "type_bridge_migration_plan_entry_operation_count",
    "type_bridge_migration_plan_entry_transaction_group_count",
    "type_bridge_migration_plan_entry_backfill_count",
    "type_bridge_migration_plan_entry_close",
    "type_bridge_migration_approval_builder_approve",
    "type_bridge_migration_approval_builder_finish",
    "type_bridge_migration_approval_builder_close",
    "type_bridge_migration_approval_set_count",
    "type_bridge_migration_approval_set_close",
    "type_bridge_migration_execution_outcome_direction",
    "type_bridge_migration_execution_outcome_status",
    "type_bridge_migration_execution_outcome_migration_identity",
    "type_bridge_migration_execution_outcome_position_kind",
    "type_bridge_migration_execution_outcome_position_ordinal",
    "type_bridge_migration_execution_outcome_diagnostics",
    "type_bridge_migration_execution_outcome_backfill_count",
    "type_bridge_migration_execution_outcome_backfill_at",
    "type_bridge_migration_execution_outcome_close",
    "type_bridge_migration_backfill_observation_identity",
    "type_bridge_migration_backfill_observation_operation_ordinal",
    "type_bridge_migration_backfill_observation_manifest_step_index",
    "type_bridge_migration_backfill_observation_plan_digest",
    "type_bridge_migration_backfill_observation_direction",
    "type_bridge_migration_backfill_observation_counts",
    "type_bridge_migration_backfill_observation_close",
    "type_bridge_migration_catalog_verify",
    "type_bridge_migration_verification_report_is_clean",
    "type_bridge_migration_verification_report_finding_count",
    "type_bridge_migration_verification_report_finding_at",
    "type_bridge_migration_verification_report_frontier_count",
    "type_bridge_migration_verification_report_frontier_at",
    "type_bridge_migration_verification_report_close",
    "type_bridge_migration_verification_finding_kind",
    "type_bridge_migration_verification_finding_pending_count",
    "type_bridge_migration_verification_finding_pending_at",
    "type_bridge_migration_verification_finding_diagnostics",
    "type_bridge_migration_verification_finding_close",
];

fn implementation_exports() -> BTreeSet<&'static str> {
    include_str!("../src/migration_abi.rs")
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub unsafe extern \"C\" fn ")
                .and_then(|rest| rest.split_once('(').map(|(name, _)| name))
        })
        .collect()
}

#[test]
fn candidate_additive_export_inventory_is_exact_and_duplicate_free() {
    let expected = ABI_1_5_CANDIDATE_ADDITIONS
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), ABI_1_5_CANDIDATE_ADDITIONS.len());
    assert_eq!(implementation_exports(), expected);
}

#[test]
fn candidate_options_layout_is_fixed_on_supported_64_bit_targets() {
    #[repr(C)]
    struct MigrationExecutionOptionsV1 {
        struct_size: u64,
        version: u32,
        flags: u32,
        timeout_milliseconds: u64,
        max_transaction_groups: u64,
        max_backfill_observations: u64,
        cancellation: *const std::ffi::c_void,
    }

    assert_eq!(
        std::mem::size_of::<usize>(),
        8,
        "ABI 1.5 target must be 64-bit"
    );
    assert_eq!(std::mem::size_of::<MigrationExecutionOptionsV1>(), 48);
    assert_eq!(std::mem::align_of::<MigrationExecutionOptionsV1>(), 8);
}
