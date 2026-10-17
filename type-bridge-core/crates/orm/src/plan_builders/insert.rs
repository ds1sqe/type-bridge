//! Insert plans.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::v3::{
    DocumentRole, InsertThing, Operation, Stage, ThingDocument, WriteReturns,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, PlanDraft, ValueTransport};

/// A relation insert whose players are references resolved in the same
/// pipeline, answering the new relation's hydration document (with the
/// players of `document_roles`, in that key order). Zero documents mean a
/// reference matched nothing and nothing was written.
pub(crate) fn relation_insert(
    authority: &PlanAuthority,
    transport: ValueTransport,
    relation: &TypeId,
    fields: &[(OwnsFactId, CanonicalValue)],
    players: &[(RoleId, Reference<'_>)],
    document_roles: Vec<DocumentRole>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(transport);
    // Declared first, so the relation column is always `relation0`.
    let thing = draft.bind("relation")?;
    let mut patterns = Vec::new();
    let mut bound = Vec::with_capacity(players.len());
    for (role, reference) in players {
        bound.push((
            role.clone(),
            reference_binding(&mut draft, reference, &mut patterns)?,
        ));
    }
    let fields = fields
        .iter()
        .map(|(field, value)| Ok((field.clone(), draft.value(value.clone())?)))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    draft.finish(
        authority,
        vec![
            Stage::Match { patterns },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: thing,
                    type_id: relation.clone(),
                    fields,
                    players: bound,
                }],
            },
            Stage::Fetch(ThingDocument::new(thing, document_roles)),
        ],
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        QueryOutput::Rows {
            columns: Vec::new(),
        },
    )
}
