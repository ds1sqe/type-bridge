//! Insert plans.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::v3::{InsertThing, Operation, Stage, WriteReturns};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, PlanDraft, ValueTransport};

/// A relation insert plan and the row columns it answers with.
#[derive(Debug)]
pub(crate) struct RelationInsertPlan {
    pub(crate) plan: BuiltPlan,
    /// Column of the new relation.
    pub(crate) relation: String,
    /// Column of each resolved player, in reference order.
    pub(crate) players: Vec<(RoleId, String)>,
}

/// A relation insert whose players are references resolved in the same
/// pipeline. Each row carries the new relation and every resolved player,
/// so zero rows mean a reference matched nothing and nothing was written,
/// and the caller can see two references resolving to one player.
pub(crate) fn relation_insert(
    authority: &PlanAuthority,
    transport: ValueTransport,
    relation: &TypeId,
    fields: &[(OwnsFactId, CanonicalValue)],
    players: &[(RoleId, Reference<'_>)],
) -> Result<RelationInsertPlan, Diagnostic> {
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
    let columns: Vec<BindingId> = std::iter::once(thing)
        .chain(bound.iter().map(|(_, player)| *player))
        .collect();
    let names = draft.names(&columns);
    let plan = draft.finish(
        authority,
        vec![
            Stage::Match { patterns },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: thing,
                    type_id: relation.clone(),
                    fields,
                    players: bound.clone(),
                }],
            },
        ],
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        QueryOutput::Rows { columns },
    )?;
    let mut names = names.into_iter();
    let relation = names.next().expect("the relation column");
    Ok(RelationInsertPlan {
        plan,
        relation,
        players: bound.into_iter().map(|(role, _)| role).zip(names).collect(),
    })
}
