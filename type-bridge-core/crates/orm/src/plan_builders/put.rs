//! Put plans.
//!
//! A keyed put matches the record of exactly the declared type by its key,
//! or creates it with the key only (`put`), then replaces every non-key
//! field in the same pipeline: given single-valued fields are updated,
//! fields the input omits are deleted. That is the helper's replace-on-put
//! semantics, which a whole-pattern TypeQL `put` does not have: it would
//! insert a duplicate key whenever the stored fields differ from the input.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::v3::{
    DeleteItem, InsertThing, Operation, Set, Stage, ThingDocument, WriteReturns,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;

use super::authority::PlanAuthority;
use super::{BuiltPlan, PlanDraft, ValueTransport};

/// One keyed entity put: `keys` and single-valued `fields` are given
/// values; `cleared` are the non-key ownerships the input omits.
pub(crate) struct KeyedPut<'a> {
    pub(crate) type_id: &'a TypeId,
    pub(crate) keys: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) cleared: &'a [OwnsFactId],
}

/// Build the keyed put. It answers the thing's hydration document once: an
/// ownership deletion repeats the row per deleted value, so the rows are
/// made distinct before the fetch.
pub(crate) fn keyed_entity_put(
    authority: &PlanAuthority,
    transport: ValueTransport,
    put: &KeyedPut<'_>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(transport);
    let thing = draft.bind("thing")?;
    let keys = put
        .keys
        .iter()
        .map(|(field, value)| Ok((field.clone(), draft.value(value.clone())?)))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut pipeline = vec![Stage::Put {
        things: vec![InsertThing {
            binding: thing,
            type_id: put.type_id.clone(),
            fields: keys,
            players: Vec::new(),
        }],
    }];
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
    pipeline.push(Stage::Select {
        bindings: vec![thing],
    });
    pipeline.push(Stage::Distinct);
    pipeline.push(Stage::Fetch(ThingDocument::new(thing, Vec::new())));
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
