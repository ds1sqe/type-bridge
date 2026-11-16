//! Single-thing insert, update and delete plans.
//!
//! Each builds one logical plan. A returning delete captures its hydration
//! document (`Stage::Fetch`) before the deletion statement in the same transaction.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::query_plan::{
    AttachItem, DeleteItem, DocumentRole, InsertThing, Operation, Pattern, Stage, ThingDocument,
    WriteReturns,
};
use type_bridge_contract::query_plan::{QueryOutput, QueryPattern};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, ValueTransport};

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
    let mut draft = authority.draft(transport);
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

#[derive(Clone, Copy)]
pub(crate) enum UpdateSelection<'a> {
    Iid(&'a str),
    Filter(&'a crate::ProjectedManagerFilter),
}

/// Replace every selected thing of exactly `type_id`: every listed
/// ownership and role is cleared, then the given values and players are
/// attached. This is the helpers' complete replacement.
pub(crate) struct ThingUpdate<'a> {
    pub(crate) type_id: &'a TypeId,
    pub(crate) selection: UpdateSelection<'a>,
    pub(crate) keys: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) cleared_fields: &'a [OwnsFactId],
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) cleared_roles: &'a [RoleId],
    pub(crate) players: &'a [(RoleId, Reference<'a>)],
    pub(crate) document_roles: Vec<DocumentRole>,
}

/// Build the update. Zero documents mean the thing (or a reference) did not
/// match, and nothing was written.
pub(crate) fn update(
    authority: &PlanAuthority,
    transport: ValueTransport,
    update: ThingUpdate<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(transport);
    let thing = draft.bind("thing")?;
    let mut patterns: Vec<Pattern> = vec![
        QueryPattern::Isa {
            binding: thing,
            include_subtypes: false,
            type_id: update.type_id.clone(),
        }
        .into(),
    ];
    if !update.keys.is_empty() {
        match update.selection {
            UpdateSelection::Iid(_) => {
                for (field, expected) in update.keys {
                    let value = draft.bind("key_guard")?;
                    patterns.push(
                        QueryPattern::Has {
                            owner: thing,
                            attribute: value,
                            attribute_id: field.attribute().clone(),
                        }
                        .into(),
                    );
                    patterns.push(
                        QueryPattern::Value {
                            comparator:
                                type_bridge_contract::migration_assertion::ValueComparator::Equal,
                            left: type_bridge_contract::query_plan::QueryOperand::Binding {
                                binding: value,
                            },
                            right: draft.value(expected.clone())?,
                        }
                        .into(),
                    );
                }
            }
            UpdateSelection::Filter(filter) => {
                let candidate = draft.bind("key_guard")?;
                let mut mismatched = vec![
                    QueryPattern::Isa {
                        binding: candidate,
                        include_subtypes: false,
                        type_id: update.type_id.clone(),
                    }
                    .into(),
                ];
                filter.append_write_predicates(&mut draft, candidate, &mut mismatched)?;
                mismatched.push(mismatch_pattern(&mut draft, candidate, update.keys)?.into());
                let mismatched = mismatched
                    .into_iter()
                    .map(|pattern| {
                        Ok(match pattern {
                            Pattern::Read(pattern) => pattern,
                            Pattern::Iid { binding, iid } => QueryPattern::Iid {
                                binding,
                                iid: type_bridge_contract::query_plan::QueryOperand::Literal {
                                    value: CanonicalValue::String(
                                        type_bridge_contract::value::CanonicalString::new(iid)?,
                                    ),
                                },
                            },
                            Pattern::Let { .. } => {
                                unreachable!("filter predicates do not assign values")
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                patterns.push(
                    QueryPattern::Not {
                        patterns: mismatched,
                    }
                    .into(),
                );
            }
        }
    }
    let mut links = Vec::new();
    for (role, reference) in update.players {
        let player = reference_binding(&mut draft, reference, &mut patterns)?;
        links.push(AttachItem::Links {
            optional: false,
            relation: thing,
            role: role.clone(),
            player,
        });
    }
    match update.selection {
        UpdateSelection::Iid(iid) => patterns.push(Pattern::Iid {
            binding: thing,
            iid: iid.to_owned(),
        }),
        UpdateSelection::Filter(filter) => {
            filter.append_write_predicates(&mut draft, thing, &mut patterns)?
        }
    }
    let mut attached = update
        .fields
        .iter()
        .map(|(field, value)| {
            Ok(AttachItem::Has {
                optional: false,
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
    let operation = if deleted.is_empty() && attached.is_empty() {
        Operation::Rows
    } else {
        ROWS
    };
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
    draft.finish(authority, pipeline, operation, no_columns())
}

fn mismatch_pattern(
    draft: &mut super::PlanDraft,
    thing: type_bridge_contract::migration_assertion::BindingId,
    keys: &[(OwnsFactId, CanonicalValue)],
) -> Result<QueryPattern, Diagnostic> {
    let mut branches = Vec::new();
    for (field, expected) in keys {
        let value = draft.bind("key_value")?;
        branches.push(vec![
            QueryPattern::Has {
                owner: thing,
                attribute: value,
                attribute_id: field.attribute().clone(),
            },
            QueryPattern::Value {
                comparator: type_bridge_contract::migration_assertion::ValueComparator::NotEqual,
                left: type_bridge_contract::query_plan::QueryOperand::Binding { binding: value },
                right: draft.value(expected.clone())?,
            },
        ]);
    }
    Ok(QueryPattern::Or { branches })
}

/// A diagnostic read after a guarded miss, or alongside a proxy update.
pub(crate) fn mismatched_keys(
    authority: &PlanAuthority,
    transport: ValueTransport,
    type_id: &TypeId,
    selection: UpdateSelection<'_>,
    keys: &[(OwnsFactId, CanonicalValue)],
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(transport);
    let thing = draft.bind("thing")?;
    let mut patterns = vec![
        QueryPattern::Isa {
            binding: thing,
            include_subtypes: false,
            type_id: type_id.clone(),
        }
        .into(),
    ];
    match selection {
        UpdateSelection::Iid(iid) => patterns.push(Pattern::Iid {
            binding: thing,
            iid: iid.to_owned(),
        }),
        UpdateSelection::Filter(filter) => {
            filter.append_write_predicates(&mut draft, thing, &mut patterns)?
        }
    }
    patterns.push(mismatch_pattern(&mut draft, thing, keys)?.into());
    draft.finish(
        authority,
        vec![Stage::Match { patterns }],
        Operation::Exists,
        no_columns(),
    )
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
    let mut draft = authority.draft(ValueTransport::Literal);
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
