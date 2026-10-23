//! Single-thing insert, update and delete plans.
//!
//! Each builds one logical plan. A returning delete captures its hydration
//! document (`Stage::Fetch`) before the deletion statement in the same transaction.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, DocumentRole, InsertThing, Operation, Pattern, Stage, ThingDocument,
    WriteReturns,
};
use type_bridge_contract::query_plan::{QueryOutput, QueryPattern};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, PlanDraft, ValueTransport};

const ROWS: Operation = Operation::Write {
    returns: WriteReturns::Rows,
};

fn no_columns() -> QueryOutput {
    QueryOutput::Rows {
        columns: Vec::new(),
    }
}

/// Insert one entity with its fields and answer its document.
pub(crate) fn entity_insert(
    authority: &PlanAuthority,
    transport: ValueTransport,
    type_id: &TypeId,
    fields: &[(OwnsFactId, CanonicalValue)],
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(transport);
    let thing = draft.bind("thing")?;
    let fields = fields
        .iter()
        .map(|(field, value)| Ok((field.clone(), draft.value(value.clone())?)))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    draft.finish(
        authority,
        vec![
            Stage::Insert {
                things: vec![InsertThing {
                    binding: thing,
                    type_id: type_id.clone(),
                    fields,
                    players: Vec::new(),
                }],
            },
            Stage::Fetch(ThingDocument::new(thing, Vec::new())),
        ],
        ROWS,
        no_columns(),
    )
}

/// One update of the thing of exactly `type_id` at `iid`: every listed
/// ownership and role is cleared, then the given values and players are
/// attached. This is the helpers' complete replacement.
pub(crate) struct ThingUpdate<'a> {
    pub(crate) type_id: &'a TypeId,
    pub(crate) iid: &'a str,
    pub(crate) cleared_fields: &'a [OwnsFactId],
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) cleared_roles: &'a [RoleId],
    pub(crate) players: &'a [(RoleId, Reference<'a>)],
    pub(crate) document_roles: Vec<DocumentRole>,
}

/// Build the update. Zero documents mean the thing (or a reference) did not
/// match, and nothing was written.
pub(crate) fn update_by_iid(
    authority: &PlanAuthority,
    transport: ValueTransport,
    update: ThingUpdate<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(transport);
    let thing = draft.bind("thing")?;
    let mut patterns: Vec<Pattern> = vec![
        QueryPattern::Isa {
            binding: thing,
            include_subtypes: false,
            type_id: update.type_id.clone(),
        }
        .into(),
    ];
    let mut links = Vec::new();
    for (role, reference) in update.players {
        let player = reference_binding(&mut draft, reference, &mut patterns)?;
        links.push(AttachItem::Links {
            relation: thing,
            role: role.clone(),
            player,
        });
    }
    patterns.push(Pattern::Iid {
        binding: thing,
        iid: update.iid.to_owned(),
    });
    let mut attached = update
        .fields
        .iter()
        .map(|(field, value)| {
            Ok(AttachItem::Has {
                owner: thing,
                field: field.clone(),
                operand: draft.value(value.clone())?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    attached.extend(links);
    let deleted: Vec<DeleteItem> = update
        .cleared_fields
        .iter()
        .map(|field| DeleteItem::Has {
            owner: thing,
            field: field.clone(),
        })
        .chain(update.cleared_roles.iter().map(|role| DeleteItem::Players {
            relation: thing,
            role: role.clone(),
        }))
        .collect();
    let mut pipeline = vec![Stage::Match { patterns }];
    if !deleted.is_empty() {
        pipeline.push(Stage::Delete { items: deleted });
    }
    if !attached.is_empty() {
        pipeline.push(Stage::Attach { items: attached });
    }
    // Clearing deletes repeat the row per old value and player; one row per
    // thing before the fetch keeps the answer to one document set.
    pipeline.push(Stage::Select {
        bindings: vec![thing],
    });
    pipeline.push(Stage::Distinct);
    pipeline.push(Stage::Fetch(ThingDocument::new(
        thing,
        update.document_roles,
    )));
    draft.finish(authority, pipeline, ROWS, no_columns())
}

/// Delete the thing of exactly `type_id` at `iid`; absent is not an error.
pub(crate) fn delete_by_iid(
    authority: &PlanAuthority,
    type_id: &TypeId,
    iid: &str,
) -> Result<BuiltPlan, Diagnostic> {
    delete_plan(authority, type_id, iid, None)
}

/// Capture the complete requested evidence before deleting an exact IID.
pub(crate) fn delete_by_iid_returning(
    authority: &PlanAuthority,
    type_id: &TypeId,
    iid: &str,
    roles: Vec<DocumentRole>,
) -> Result<BuiltPlan, Diagnostic> {
    delete_plan(authority, type_id, iid, Some(roles))
}

fn delete_plan(
    authority: &PlanAuthority,
    type_id: &TypeId,
    iid: &str,
    roles: Option<Vec<DocumentRole>>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(ValueTransport::Literal);
    let thing = draft.bind("thing")?;
    let mut pipeline = vec![Stage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: thing,
                include_subtypes: false,
                type_id: type_id.clone(),
            }
            .into(),
            Pattern::Iid {
                binding: thing,
                iid: iid.to_owned(),
            },
        ],
    }];
    let operation = if let Some(roles) = roles {
        pipeline.push(Stage::Fetch(ThingDocument::new(thing, roles)));
        ROWS
    } else {
        Operation::Write {
            returns: WriteReturns::Matched,
        }
    };
    pipeline.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: thing }],
    });
    draft.finish(authority, pipeline, operation, no_columns())
}
