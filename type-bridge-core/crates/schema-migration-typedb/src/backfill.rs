//! Closed TypeQL lowering for canonical attribute backfill plans.
//!
//! These queries are internal provider implementation details. Every label and
//! batch bound comes from a validated canonical plan; callers cannot supply
//! TypeQL fragments or alter the fixed conflict and postcondition semantics.

use type_bridge_contract::migration_backfill::AttributeBackfillPlan;
use type_bridge_core_lib::__internal as tql;
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

    fn source_patterns(
        &self,
        with_destination: bool,
        with_stable: bool,
    ) -> Result<Vec<tql::Pattern>, tql::TqlError> {
        let mut constraints = vec![
            tql::isa(self.plan.owner().label().as_str(), false)?,
            tql::has(self.plan.source().label().as_str(), tql::var("source")?)?,
        ];
        if with_destination {
            constraints.push(tql::has(
                self.plan.destination().label().as_str(),
                tql::var("destination")?,
            )?);
        }
        if with_stable {
            constraints.push(tql::has(
                self.plan.partition().stable_attribute().label().as_str(),
                tql::var("stable")?,
            )?);
        }
        Ok(vec![tql::thing("owner", constraints)?])
    }

    fn equal_destination(&self) -> Result<Vec<tql::Pattern>, tql::TqlError> {
        Ok(vec![
            tql::thing(
                "owner",
                vec![tql::has(
                    self.plan.destination().label().as_str(),
                    tql::var("destination")?,
                )?],
            )?,
            tql::compare(tql::var("destination")?, "==", tql::var("source")?)?,
        ])
    }

    /// Return a bounded witness query for a value that makes execution unsafe.
    pub(crate) fn conflict_witness(&self) -> Result<Option<String>, tql::TqlError> {
        if self.direction == BackfillExecutionDirection::Reverse {
            return Ok(None);
        }
        let mut patterns = self.source_patterns(true, false)?;
        patterns.push(tql::compare(
            tql::var("destination")?,
            "!=",
            tql::var("source")?,
        )?);
        Ok(Some(tql::render(&tql::pipeline(vec![
            tql::match_stage(patterns),
            tql::limit(1),
            tql::fetch(vec![(
                "conflict",
                tql::fetch_expr(tql::var("destination")?),
            )])?,
        ]))?))
    }

    /// Return a bounded witness query proving the terminal predicate is false.
    pub(crate) fn incomplete_witness(&self) -> Result<String, tql::TqlError> {
        let reverse = self.direction == BackfillExecutionDirection::Reverse;
        let mut patterns = self.source_patterns(reverse, false)?;
        let value = if reverse {
            patterns.push(tql::compare(
                tql::var("destination")?,
                "==",
                tql::var("source")?,
            )?);
            "destination"
        } else {
            patterns.push(tql::not(self.equal_destination()?));
            "source"
        };
        tql::render(&tql::pipeline(vec![
            tql::match_stage(patterns),
            tql::limit(1),
            tql::fetch(vec![("incomplete", tql::fetch_expr(tql::var(value)?))])?,
        ]))
    }

    /// Return one deterministic bounded mutation pipeline.
    pub(crate) fn mutation_group(&self) -> Result<String, tql::TqlError> {
        let reverse = self.direction == BackfillExecutionDirection::Reverse;
        let mut patterns = self.source_patterns(reverse, true)?;
        let mutation = if reverse {
            patterns.push(tql::compare(
                tql::var("destination")?,
                "==",
                tql::var("source")?,
            )?);
            tql::delete(vec![tql::delete_has("owner", "destination")?])
        } else {
            patterns.push(tql::not(self.equal_destination()?));
            tql::insert(vec![tql::thing(
                "owner",
                vec![tql::has_comparison(
                    self.plan.destination().label().as_str(),
                    "==",
                    tql::var("source")?,
                )?],
            )?])
        };
        tql::render(&tql::pipeline(vec![
            tql::match_stage(patterns),
            tql::sort_default(&["stable"])?,
            tql::limit(u64::from(self.plan.partition().batch_rows())),
            mutation,
            tql::fetch(vec![(
                "changed_partition",
                tql::fetch_expr(tql::var("stable")?),
            )])?,
        ]))
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
        assert!(tql::equivalent(
            &lowered.conflict_witness().unwrap().unwrap(),
            "match\n  $owner isa person, has old-name $source, has display-name $destination;\n  $destination != $source;\nlimit 1;\nfetch { \"conflict\": $destination };"
        ).unwrap());
        let mutation = lowered.mutation_group().unwrap();
        assert!(mutation.contains("not { $owner has display-name $destination;"));
        assert!(mutation.contains("sort $stable; limit 37; insert"));
        assert!(mutation.contains("$owner has display-name == $source;"));
    }

    #[test]
    fn reverse_lowering_removes_only_an_equal_copy() {
        let plan = plan();
        let lowered = LoweredBackfill::new(&plan, BackfillExecutionDirection::Reverse);
        assert!(lowered.conflict_witness().unwrap().is_none());
        assert!(
            lowered
                .incomplete_witness()
                .unwrap()
                .contains("$destination == $source;")
        );
        let mutation = lowered.mutation_group().unwrap();
        assert!(mutation.contains("sort $stable; limit 37; delete"));
        assert!(mutation.contains("has $destination of $owner;"));
    }
}
