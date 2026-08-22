//! Closed TypeQL lowering for canonical attribute backfill plans.
//!
//! These queries are internal provider implementation details. Every label and
//! batch bound comes from a validated canonical plan; callers cannot supply
//! TypeQL fragments or alter the fixed conflict and postcondition semantics.

use type_bridge_contract::migration_backfill::AttributeBackfillPlan;
use type_bridge_schema_migration::BackfillExecutionDirection;

pub(crate) struct LoweredBackfill<'a> {
    plan: &'a AttributeBackfillPlan,
    direction: BackfillExecutionDirection,
}

impl<'a> LoweredBackfill<'a> {
    pub(crate) const fn new(
        plan: &'a AttributeBackfillPlan,
        direction: BackfillExecutionDirection,
    ) -> Self {
        Self { plan, direction }
    }

    /// Return a bounded witness query for a value that makes execution unsafe.
    pub(crate) fn conflict_witness(&self) -> Option<String> {
        match self.direction {
            BackfillExecutionDirection::Forward => Some(format!(
                "match\n  $owner isa {owner}, has {source} $source, has {destination} $destination;\n  $destination != $source;\nlimit 1;\nfetch {{ \"conflict\": $destination }};",
                owner = self.plan.owner().label(),
                source = self.plan.source().label(),
                destination = self.plan.destination().label(),
            )),
            BackfillExecutionDirection::Reverse => None,
        }
    }

    /// Return a bounded witness query proving the terminal predicate is false.
    pub(crate) fn incomplete_witness(&self) -> String {
        match self.direction {
            BackfillExecutionDirection::Forward => format!(
                "match\n  $owner isa {owner}, has {source} $source;\n  not {{ $owner has {destination} $destination; $destination == $source; }};\nlimit 1;\nfetch {{ \"incomplete\": $source }};",
                owner = self.plan.owner().label(),
                source = self.plan.source().label(),
                destination = self.plan.destination().label(),
            ),
            BackfillExecutionDirection::Reverse => format!(
                "match\n  $owner isa {owner}, has {source} $source, has {destination} $destination;\n  $destination == $source;\nlimit 1;\nfetch {{ \"incomplete\": $destination }};",
                owner = self.plan.owner().label(),
                source = self.plan.source().label(),
                destination = self.plan.destination().label(),
            ),
        }
    }

    /// Return one deterministic bounded mutation pipeline.
    pub(crate) fn mutation_group(&self) -> String {
        let batch_rows = self.plan.partition().batch_rows();
        let stable = self.plan.partition().stable_attribute().label();
        match self.direction {
            BackfillExecutionDirection::Forward => format!(
                "match\n  $owner isa {owner}, has {source} $source, has {stable} $stable;\n  not {{ $owner has {destination} $destination; $destination == $source; }};\nsort $stable;\nlimit {batch_rows};\ninsert\n  $owner has {destination} == $source;\nfetch {{ \"changed_partition\": $stable }};",
                owner = self.plan.owner().label(),
                source = self.plan.source().label(),
                destination = self.plan.destination().label(),
            ),
            BackfillExecutionDirection::Reverse => format!(
                "match\n  $owner isa {owner}, has {source} $source, has {destination} $destination, has {stable} $stable;\n  $destination == $source;\nsort $stable;\nlimit {batch_rows};\ndelete\n  has $destination of $owner;\nfetch {{ \"changed_partition\": $stable }};",
                owner = self.plan.owner().label(),
                source = self.plan.source().label(),
                destination = self.plan.destination().label(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_backfill::{BackfillPartition, BackfillReverseProgram};
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;

    fn plan() -> AttributeBackfillPlan {
        AttributeBackfillPlan::new(
            TypeId::new(TypeKind::Entity, "person").unwrap(),
            AttributeId::new("old-name").unwrap(),
            AttributeId::new("display-name").unwrap(),
            BackfillPartition::new(37, AttributeId::new("person-id").unwrap()).unwrap(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
                b"schema",
            )
            .unwrap(),
            Some(BackfillReverseProgram::RemoveEqualCopiedDestination),
        )
        .unwrap()
    }

    #[test]
    fn forward_lowering_is_closed_bounded_and_idempotent() {
        let plan = plan();
        let lowered = LoweredBackfill::new(&plan, BackfillExecutionDirection::Forward);
        assert_eq!(
            lowered.conflict_witness().unwrap(),
            "match\n  $owner isa person, has old-name $source, has display-name $destination;\n  $destination != $source;\nlimit 1;\nfetch { \"conflict\": $destination };"
        );
        let mutation = lowered.mutation_group();
        assert!(mutation.contains("not { $owner has display-name $destination;"));
        assert!(mutation.contains("sort $stable;\nlimit 37;\ninsert"));
        assert!(mutation.contains("$owner has display-name == $source;"));
    }

    #[test]
    fn reverse_lowering_removes_only_an_equal_copy() {
        let plan = plan();
        let lowered = LoweredBackfill::new(&plan, BackfillExecutionDirection::Reverse);
        assert!(lowered.conflict_witness().is_none());
        assert!(
            lowered
                .incomplete_witness()
                .contains("$destination == $source;")
        );
        let mutation = lowered.mutation_group();
        assert!(mutation.contains("sort $stable;\nlimit 37;\ndelete"));
        assert!(mutation.contains("has $destination of $owner;"));
    }
}
