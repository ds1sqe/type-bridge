//! Role-player references lowered into the write plan's own match.
//!
//! A reference names a thing by IID or by one key value and admits one or
//! more concrete candidate types. One candidate is an exact `isa!` binding
//! with the identity predicate; several candidates are a disjunction with
//! one branch per type. Zero matching things leave the write with zero rows,
//! so a missing reference writes nothing and needs no prior read.

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{AttributeId, TypeId};
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::v3::Pattern;
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::value::CanonicalValue;

use super::PlanDraft;

/// How a reference identifies its thing.
pub(crate) enum ReferenceIdentity<'a> {
    /// Canonical provider IID.
    Iid(&'a str),
    /// One key ownership value.
    Key {
        attribute: &'a AttributeId,
        value: CanonicalValue,
    },
}

/// One reference: its identity and the concrete types it may resolve to.
pub(crate) struct Reference<'a> {
    pub(crate) candidates: &'a [TypeId],
    pub(crate) identity: ReferenceIdentity<'a>,
}

/// Bind one reference and append the patterns that resolve it.
pub(crate) fn reference_binding(
    draft: &mut PlanDraft,
    reference: &Reference<'_>,
    patterns: &mut Vec<Pattern>,
) -> Result<BindingId, Diagnostic> {
    let player = draft.bind("player")?;
    let key = match &reference.identity {
        ReferenceIdentity::Iid(_) => None,
        ReferenceIdentity::Key { attribute, value } => {
            Some((*attribute, draft.value(value.clone())?))
        }
    };
    let mut branches = Vec::with_capacity(reference.candidates.len());
    for type_id in reference.candidates {
        let mut branch = vec![QueryPattern::Isa {
            binding: player,
            include_subtypes: false,
            type_id: type_id.clone(),
        }];
        if let Some((attribute, operand)) = &key {
            // Each branch binds its own key attribute; only the player is
            // established by every branch.
            let value = draft.bind("key")?;
            branch.push(QueryPattern::Has {
                attribute: value,
                attribute_id: (*attribute).clone(),
                owner: player,
            });
            branch.push(QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding { binding: value },
                right: operand.clone(),
            });
        }
        branches.push(branch);
    }
    if let [branch] = branches.as_mut_slice() {
        patterns.extend(branch.drain(..).map(Pattern::Read));
    } else {
        patterns.push(Pattern::Read(QueryPattern::Or { branches }));
    }
    if let ReferenceIdentity::Iid(iid) = reference.identity {
        patterns.push(Pattern::Iid {
            binding: player,
            iid: iid.to_owned(),
        });
    }
    Ok(player)
}
