//! Proof that a deferred relation player is present before allocating its seed.
use super::{Pattern, PlanSpec, Stage};
use crate::id::{AttributeId, TypeId};
use crate::migration_assertion::{BindingId, ValueComparator};
use crate::query_plan::{InputColumnId, QueryOperand, QueryPattern};
use std::collections::BTreeSet;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Route {
    type_id: TypeId,
    subtypes: bool,
    key: Option<AttributeId>,
    input: InputColumnId,
}

fn input(operand: &QueryOperand) -> Option<InputColumnId> {
    match operand {
        QueryOperand::Input { column } => Some(*column),
        _ => None,
    }
}

// An optional binding must be exactly this lookup: extra predicates could make
// it absent even when a looser existence witness succeeds.
fn route(patterns: &[QueryPattern], binding: BindingId, exact: bool) -> Option<Route> {
    let (type_id, subtypes) = patterns.iter().find_map(|pattern| match pattern {
        QueryPattern::Isa {
            binding: owner,
            type_id,
            include_subtypes,
        } if *owner == binding => Some((type_id.clone(), *include_subtypes)),
        _ => None,
    })?;
    if (!exact || patterns.len() == 2)
        && let Some(column) = patterns.iter().find_map(|pattern| match pattern {
            QueryPattern::Iid {
                binding: owner,
                iid,
            } if *owner == binding => input(iid),
            _ => None,
        })
    {
        return Some(Route {
            type_id,
            subtypes,
            key: None,
            input: column,
        });
    }
    if exact && patterns.len() != 3 {
        return None;
    }
    patterns.iter().find_map(|pattern| {
        let QueryPattern::Has {
            attribute,
            owner,
            attribute_id,
        } = pattern
        else {
            return None;
        };
        if *owner != binding {
            return None;
        }
        let column = patterns.iter().find_map(|pattern| match pattern {
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding { binding },
                right,
            } if binding == attribute => input(right),
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                right: QueryOperand::Binding { binding },
                left,
            } if binding == attribute => input(left),
            _ => None,
        })?;
        Some(Route {
            type_id: type_id.clone(),
            subtypes,
            key: Some(attribute_id.clone()),
            input: column,
        })
    })
}

fn conjunctions<'a>(patterns: &'a [QueryPattern], output: &mut Vec<&'a QueryPattern>) {
    for pattern in patterns {
        if let QueryPattern::Or { branches } = pattern
            && branches.len() == 1
        {
            conjunctions(&branches[0], output);
        } else {
            output.push(pattern);
        }
    }
}

fn absent_input(patterns: &[QueryPattern]) -> Option<InputColumnId> {
    let [QueryPattern::Not { patterns }] = patterns else {
        return None;
    };
    let [
        QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left,
            right,
        },
    ] = patterns.as_slice()
    else {
        return None;
    };
    let column = input(left)?;
    (input(right) == Some(column)).then_some(column)
}

fn guarded_column(
    branches: &[Vec<QueryPattern>],
    sources: &BTreeSet<Route>,
) -> Option<InputColumnId> {
    let mut column = None;
    let mut witness = false;
    for branch in branches {
        let current = if let Some(column) = absent_input(branch) {
            column
        } else {
            let route = branch.iter().find_map(|pattern| match pattern {
                QueryPattern::Isa { binding, .. } => {
                    route(branch, *binding, false).filter(|route| sources.contains(route))
                }
                _ => None,
            })?;
            witness = true;
            route.input
        };
        if column.is_some_and(|previous| previous != current) {
            return None;
        }
        column = Some(current);
    }
    witness.then_some(column).flatten()
}

pub(super) fn players_proven(
    spec: &PlanSpec,
    stage_index: usize,
    relation: BindingId,
    mandatory: &BTreeSet<BindingId>,
    optional: &BTreeSet<BindingId>,
) -> bool {
    if spec.pipeline[stage_index + 1..].iter().any(|stage| {
        matches!(stage, Stage::Attach { items } if items.iter().any(|item| matches!(item,
            super::AttachItem::Links { relation: owner, player, .. }
                if *owner == relation && mandatory.contains(player))))
    }) {
        return true;
    }
    let Some(rows) = spec.given_rows.as_ref().filter(|rows| !rows.is_empty()) else {
        return false;
    };
    let players = spec.pipeline[stage_index + 1..]
        .iter()
        .flat_map(|stage| match stage {
            Stage::Attach { items } => items.as_slice(),
            _ => &[],
        })
        .filter_map(|item| match item {
            super::AttachItem::Links {
                relation: owner,
                player,
                optional: true,
                ..
            } if *owner == relation && optional.contains(player) => Some(*player),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut covered = BTreeSet::new();
    for stage in &spec.pipeline[..stage_index] {
        if stage.is_write() {
            covered.clear();
            continue;
        }
        let Stage::Match { patterns } = stage else {
            continue;
        };
        let reads = patterns
            .iter()
            .filter_map(|pattern| match pattern {
                Pattern::Read(pattern) => Some(pattern.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut clauses = Vec::new();
        conjunctions(&reads, &mut clauses);
        let mut sources = BTreeSet::new();
        for clause in &clauses {
            if let QueryPattern::Try { patterns } = clause {
                for player in players.intersection(optional) {
                    if let Some(route) = route(patterns, *player, true) {
                        sources.insert(route);
                    }
                }
            }
        }
        // Witness and optional lookup share one match snapshot. A witness from
        // a later match could observe a newly-created player the earlier Try missed.
        for clause in clauses {
            if let QueryPattern::Or { branches } = clause
                && let Some(column) = guarded_column(branches, &sources)
            {
                covered.insert(column);
            }
        }
    }
    rows.iter().all(|row| {
        covered.iter().any(|column| {
            row.values()
                .get(usize::from(column.get()))
                .is_some_and(Option::is_some)
        })
    })
}
