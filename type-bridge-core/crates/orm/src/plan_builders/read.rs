//! Identity reads over the same schema authority as mutation plans.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::v3::{Expr, Operation, Pattern, Stage};

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, PlanDraft, ValueTransport};

/// Resolve a reference without loading the thing's attributes or players.
pub(crate) fn reference_identity(
    authority: &PlanAuthority,
    transport: ValueTransport,
    reference: &Reference<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(transport);
    let mut patterns = Vec::new();
    let thing = reference_binding(&mut draft, reference, &mut patterns)?;
    let iid = draft.bind("iid")?;
    patterns.push(Pattern::Let {
        binding: iid,
        expr: Expr::Iid { binding: thing },
    });
    draft.finish(
        authority,
        vec![Stage::Match { patterns }],
        Operation::Rows,
        QueryOutput::Rows { columns: vec![iid] },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;

    #[test]
    fn identity_lookup_projects_only_the_iid() {
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"identity-read",
            )
            .unwrap(),
        );
        let plan = reference_identity(
            &authority,
            ValueTransport::Given,
            &Reference {
                candidates: &[TypeId::new(TypeKind::Entity, "doctor").unwrap()],
                identity: super::super::reference::ReferenceIdentity::Iid(
                    "0x1e00000000000000000001",
                ),
            },
        )
        .unwrap();
        let lowered = crate::query_plan_exec::lower(plan.local()).unwrap();
        let source = type_bridge_core_lib::__internal::render(&lowered.query).unwrap();
        assert!(source.contains("iid($player0)"), "{source}");
        assert!(!source.contains("attributes"), "{source}");
        assert!(
            matches!(plan.local().output(), QueryOutput::Rows { columns } if columns.len() == 1)
        );
    }

    #[test]
    fn relation_reference_lookup_is_exact_and_keeps_keys_out_of_typeql() {
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"reference-read",
            )
            .unwrap(),
        );
        let attribute = type_bridge_contract::id::AttributeId::new("order-id").unwrap();
        let plan = reference_identity(
            &authority,
            ValueTransport::Given,
            &Reference {
                candidates: &[TypeId::new(TypeKind::Relation, "purchase").unwrap()],
                identity: super::super::reference::ReferenceIdentity::Key {
                    attribute: &attribute,
                    value: type_bridge_contract::value::CanonicalValue::String(
                        type_bridge_contract::value::CanonicalString::new("a\\\"b").unwrap(),
                    ),
                },
            },
        )
        .unwrap();
        let lowered = crate::query_plan_exec::lower(plan.local()).unwrap();
        let source = type_bridge_core_lib::__internal::render(&lowered.query).unwrap();
        assert!(source.contains("$player0 isa! purchase"), "{source}");
        assert!(source.contains("$key1 == $in0"), "{source}");
        assert!(!source.contains("a\\\"b"), "{source}");
        assert_eq!(
            lowered.given.unwrap().rows,
            [vec![crate::session::backend::GivenValue::String(
                "a\\\"b".into()
            )]]
        );
    }
}
