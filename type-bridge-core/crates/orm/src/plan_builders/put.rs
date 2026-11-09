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
    AttachItem, DeleteItem, DocumentRole, DocumentScalar, Expr, InsertThing, Operation, Pattern,
    Set, Stage, ThingDocument, WriteReturns,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::reference::{Reference, reference_binding};
use super::{BuiltPlan, ValueTransport};

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
        let value = i64::try_from(ordinal).map_err(|_| {
            Diagnostic::new(
                type_bridge_contract::diagnostic::DiagnosticCategory::ResourceLimit,
                type_bridge_contract::diagnostic::DiagnosticCode::new("manager_batch_limit")
                    .expect("static code"),
                "Batch ordinal exceeds its scalar domain",
            )
        })?;
        patterns.push(Pattern::Let {
            binding,
            expr: Expr::Operand(draft.value(CanonicalValue::Long(value))?),
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
