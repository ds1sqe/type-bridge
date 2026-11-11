//! Put plans.
//!
//! A keyless put is TypeQL `put` of the whole given pattern: every given
//! field value and role player. It matches an instance of exactly the
//! declared type (`isa!`: the manager hydrates one exact model, so a
//! subtype instance is never taken) or inserts the pattern; it never
//! updates.
//!
//! A keyed put matches the record of exactly the declared type by its key,
//! or creates it with the key only (`put`), then replaces every non-key
//! field in the same pipeline: given single-valued fields are updated,
//! fields the input omits are deleted. That is the helper's replace-on-put
//! semantics, which a whole-pattern TypeQL `put` does not have: it would
//! insert a duplicate key whenever the stored fields differ from the input.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::migration_assertion::QueryVariable;
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::{
    AttachItem, DeleteItem, DocumentRole, DocumentScalar, Expr, InputRow, InsertThing, Operation,
    Pattern, QueryPattern, Set, Stage, ThingDocument, WriteReturns,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, ReferenceIdentity, reference_binding};
use super::{BuiltPlan, ValueTransport};

#[path = "put_placement.rs"]
mod placement;

/// Build one keyless put answering the matched or inserted thing's
/// document. Batch rows pass their `ordinal`, answered as a document scalar.
/// The whole pattern is one statement, so a row over the plan limits is
/// refused by the plan validation, never split.
pub(crate) fn keyless_put(
    authority: &PlanAuthority,
    transport: ValueTransport,
    type_id: &TypeId,
    fields: &[(OwnsFactId, CanonicalValue)],
    players: &[(RoleId, Reference<'_>)],
    document_roles: Vec<DocumentRole>,
    ordinal: Option<usize>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(transport);
    let thing = draft.bind("root")?;
    let ordinal_binding = ordinal.map(|_| draft.bind("ordinal")).transpose()?;
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
    let mut document = ThingDocument::new(thing, document_roles);
    if let (Some(ordinal), Some(binding)) = (ordinal, ordinal_binding) {
        patterns.push(Pattern::Let {
            binding,
            expr: Expr::Operand(draft.value(ordinal_value(ordinal)?)?),
        });
        document.scalars.push(DocumentScalar {
            key: QueryVariable::new("ordinal")?,
            binding,
        });
    }
    let mut pipeline = Vec::with_capacity(3);
    if !patterns.is_empty() {
        pipeline.push(Stage::Match { patterns });
    }
    pipeline.push(Stage::Put {
        include_subtypes: false,
        things: vec![InsertThing {
            binding: thing,
            type_id: type_id.clone(),
            fields,
            players: bound,
        }],
    });
    pipeline.push(Stage::Fetch(document));
    draft.finish(
        authority,
        pipeline,
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        QueryOutput::Rows {
            columns: Vec::new(),
        },
    )
}

/// One row of a keyless `put_many`: its call-row `ordinal` and pattern.
pub(crate) struct KeylessRow<'a> {
    pub(crate) ordinal: usize,
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) players: &'a [(RoleId, Reference<'a>)],
}

/// Build the keyless puts of one `put_many`, with the answers of
/// sequential single-row TypeQL puts in call-row order.
///
/// Rows of one shape (the same fields and player routes) share a plan, one
/// `given` row each. Rows of one Put stage don't see each other's inserts,
/// so a row whose pattern may match another row's instance runs in a later
/// plan than every earlier such row, and in no earlier plan than a later
/// one. A one-row plan keeps the single-row bytes.
pub(crate) fn keyless_put_many(
    authority: &PlanAuthority,
    type_id: &TypeId,
    rows: &[KeylessRow<'_>],
    document_roles: &[DocumentRole],
) -> Result<Vec<BuiltPlan>, Diagnostic> {
    let plans = placement::place(rows);
    let mut built = Vec::with_capacity(plans.len());
    for members in plans {
        let members = members
            .iter()
            .map(|&index| &rows[index])
            .collect::<Vec<_>>();
        keyless_put_group(authority, type_id, &members, document_roles, &mut built)?;
    }
    Ok(built)
}

/// Build one shape group, halving it while its input rows exceed the plan
/// limits: consecutive halves keep call-row order.
fn keyless_put_group(
    authority: &PlanAuthority,
    type_id: &TypeId,
    rows: &[&KeylessRow<'_>],
    document_roles: &[DocumentRole],
    built: &mut Vec<BuiltPlan>,
) -> Result<(), Diagnostic> {
    if let [row] = rows {
        built.push(keyless_put(
            authority,
            ValueTransport::Given,
            type_id,
            row.fields,
            row.players,
            document_roles.to_vec(),
            Some(row.ordinal),
        )?);
        return Ok(());
    }
    match keyless_put_rows(authority, type_id, rows, document_roles.to_vec()) {
        Ok(plan) => built.push(plan),
        Err(error)
            if matches!(
                error.code().as_str(),
                "query_invocation_input_byte_limit"
                    | "query_invocation_row_limit"
                    | "canonical_json_too_large"
                    | "canonical_collection_too_large"
            ) =>
        {
            let (first, second) = rows.split_at(rows.len() / 2);
            keyless_put_group(authority, type_id, first, document_roles, built)?;
            keyless_put_group(authority, type_id, second, document_roles, built)?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

/// One Put stage over several rows of one shape: the ordinal, player
/// identities (IIDs too) and field values are input columns.
fn keyless_put_rows(
    authority: &PlanAuthority,
    type_id: &TypeId,
    rows: &[&KeylessRow<'_>],
    document_roles: Vec<DocumentRole>,
) -> Result<BuiltPlan, Diagnostic> {
    use super::bulk_write::{player_value, reference_patterns};
    let mut draft = authority.draft(ValueTransport::Given);
    let thing = draft.bind("root")?;
    let ordinal = draft.bind("ordinal")?;
    let mut columns = 0_usize;
    let mut column = |draft: &mut super::PlanDraft, value: &CanonicalValue| {
        let name = format!("in{columns}");
        columns += 1;
        draft.input(&name, value.value_type(), false)
    };
    let values = rows
        .iter()
        .map(|row| {
            let mut values = vec![Some(ordinal_value(row.ordinal)?)];
            for (role, reference) in row.players {
                values.push(Some(player_value(role, reference)?.1));
            }
            values.extend(row.fields.iter().map(|(_, value)| Some(value.clone())));
            Ok(InputRow::new(values))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let first = rows[0];
    let mut patterns = vec![Pattern::Let {
        binding: ordinal,
        expr: Expr::Operand(column(&mut draft, &ordinal_value(first.ordinal)?)?),
    }];
    let mut bound = Vec::with_capacity(first.players.len());
    for (role, reference) in first.players {
        let player = draft.bind("player")?;
        let input = column(&mut draft, &player_value(role, reference)?.1)?;
        let key = match &reference.identity {
            ReferenceIdentity::Iid(_) => None,
            ReferenceIdentity::Key { attribute, .. } => Some(*attribute),
        };
        let mut branches = reference
            .candidates
            .iter()
            .map(|candidate| reference_patterns(&mut draft, player, candidate, key, &input))
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        if let [branch] = branches.as_mut_slice() {
            patterns.extend(branch.drain(..).map(Pattern::Read));
        } else {
            patterns.push(Pattern::Read(QueryPattern::Or { branches }));
        }
        bound.push((role.clone(), player));
    }
    let fields = first
        .fields
        .iter()
        .map(|(field, value)| Ok((field.clone(), column(&mut draft, value)?)))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut document = ThingDocument::new(thing, document_roles);
    document.scalars.push(DocumentScalar {
        key: QueryVariable::new("ordinal")?,
        binding: ordinal,
    });
    draft.finish_rows(
        authority,
        vec![
            Stage::Match { patterns },
            Stage::Put {
                include_subtypes: false,
                things: vec![InsertThing {
                    binding: thing,
                    type_id: type_id.clone(),
                    fields,
                    players: bound,
                }],
            },
            Stage::Fetch(document),
        ],
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        QueryOutput::Rows {
            columns: Vec::new(),
        },
        Some(values),
    )
}

fn ordinal_value(ordinal: usize) -> Result<CanonicalValue, Diagnostic> {
    i64::try_from(ordinal)
        .map(CanonicalValue::Long)
        .map_err(|_| ordinal_overflow())
}

fn ordinal_overflow() -> Diagnostic {
    Diagnostic::new(
        type_bridge_contract::diagnostic::DiagnosticCategory::ResourceLimit,
        type_bridge_contract::diagnostic::DiagnosticCode::new("manager_batch_limit")
            .expect("static code"),
        "Batch ordinal exceeds its scalar domain",
    )
}

/// Whether `row`'s put pattern may match the instance `earlier` inserts:
/// every field value and player of `row` may equal one of `earlier`'s.
/// Undecidable equality (a key reference against an IID) counts as equal.
fn covers(earlier: &KeylessRow<'_>, row: &KeylessRow<'_>) -> bool {
    row.fields.iter().all(|(field, value)| {
        earlier
            .fields
            .iter()
            .any(|(other, candidate)| other == field && may_equal(candidate, value))
    }) && row.players.iter().all(|(role, reference)| {
        earlier.players.iter().any(|(other, candidate)| {
            other == role
                && match (&candidate.identity, &reference.identity) {
                    (ReferenceIdentity::Iid(left), ReferenceIdentity::Iid(right)) => {
                        left.eq_ignore_ascii_case(right)
                    }
                    (
                        ReferenceIdentity::Key {
                            attribute: left_attribute,
                            value: left,
                        },
                        ReferenceIdentity::Key {
                            attribute: right_attribute,
                            value: right,
                        },
                    ) if left_attribute == right_attribute => may_equal(left, right),
                    _ => true,
                }
        })
    })
}

/// Provider equality: semantic within a domain (a time zone or a decimal
/// scale doesn't make values differ); a domain without one counts as equal.
fn may_equal(left: &CanonicalValue, right: &CanonicalValue) -> bool {
    left == right
        || left
            .semantic_cmp_same_domain(right)
            .is_none_or(std::cmp::Ordering::is_eq)
}

/// One keyed entity put: `keys` and single-valued `fields` are given
/// values; `cleared` are the non-key ownerships to empty (omitted, or
/// multi-valued and given); `attached` are the given multi-valued values.
pub(crate) struct KeyedPut<'a> {
    pub(crate) include_subtypes: bool,
    pub(crate) return_document: bool,
    pub(crate) type_id: &'a TypeId,
    pub(crate) keys: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) cleared: &'a [OwnsFactId],
    pub(crate) attached: &'a [(OwnsFactId, CanonicalValue)],
}

/// Build the keyed put. It answers the thing's hydration document once: an
/// ownership deletion repeats the row per deleted value, so the rows are
/// made distinct before the fetch.
pub(crate) fn keyed_entity_put(
    authority: &PlanAuthority,
    transport: ValueTransport,
    put: &KeyedPut<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    keyed_put(authority, transport, put, None)
}

pub(crate) struct RelationPut<'a> {
    pub(crate) players: &'a [(RoleId, Reference<'a>)],
    pub(crate) cleared_roles: &'a [RoleId],
    pub(crate) document_roles: Vec<DocumentRole>,
}

pub(crate) fn keyed_relation_put(
    authority: &PlanAuthority,
    transport: ValueTransport,
    put: &KeyedPut<'_>,
    relation: RelationPut<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    keyed_put(authority, transport, put, Some(relation))
}

fn keyed_put(
    authority: &PlanAuthority,
    transport: ValueTransport,
    put: &KeyedPut<'_>,
    relation: Option<RelationPut<'_>>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(transport);
    let thing = draft.bind("thing")?;
    let mut patterns = Vec::new();
    let mut links = Vec::new();
    if let Some(relation) = &relation {
        for (role, reference) in relation.players {
            let player = reference_binding(&mut draft, reference, &mut patterns)?;
            links.push(AttachItem::Links {
                optional: false,
                relation: thing,
                role: role.clone(),
                player,
            });
        }
    }
    let keys = put
        .keys
        .iter()
        .map(|(field, value)| Ok((field.clone(), draft.value(value.clone())?)))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut pipeline = Vec::new();
    if !patterns.is_empty() {
        pipeline.push(Stage::Match { patterns });
    }
    // Put only the key: new players in its match would duplicate an existing key.
    pipeline.push(Stage::Put {
        include_subtypes: put.include_subtypes,
        things: vec![InsertThing {
            binding: thing,
            type_id: put.type_id.clone(),
            fields: keys,
            players: Vec::new(),
        }],
    });
    if let Some(relation) = &relation
        && !relation.cleared_roles.is_empty()
    {
        pipeline.push(Stage::Delete {
            items: relation
                .cleared_roles
                .iter()
                .map(|role| DeleteItem::Players {
                    relation: thing,
                    role: role.clone(),
                })
                .collect(),
        });
    }
    if !put.cleared.is_empty() {
        pipeline.push(Stage::Delete {
            items: put
                .cleared
                .iter()
                .map(|field| DeleteItem::Has {
                    owner: thing,
                    field: field.clone(),
                })
                .collect(),
        });
    }
    if !put.fields.is_empty() {
        let sets = put
            .fields
            .iter()
            .map(|(field, value)| {
                Ok(Set::Field {
                    owner: thing,
                    field: field.clone(),
                    operand: draft.value(value.clone())?,
                })
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        pipeline.push(Stage::Update { sets });
    }
    if !put.attached.is_empty() {
        let items = put
            .attached
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
        pipeline.push(Stage::Attach { items });
    }
    if !links.is_empty() {
        pipeline.push(Stage::Attach { items: links });
    }
    pipeline.push(Stage::Select {
        bindings: vec![thing],
    });
    pipeline.push(Stage::Distinct);
    if put.return_document {
        pipeline.push(Stage::Fetch(ThingDocument::new(
            thing,
            relation.map_or_else(Vec::new, |relation| relation.document_roles),
        )));
    }
    draft.finish(
        authority,
        pipeline,
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        QueryOutput::Rows {
            columns: if put.return_document {
                Vec::new()
            } else {
                vec![thing]
            },
        },
    )
}
