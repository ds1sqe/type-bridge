//! Placement of keyless `put_many` rows into plans.
//!
//! Row j may join a plan of its shape at index L or later, where L is the
//! last plan holding an earlier row related to j (either row's pattern may
//! match the other's instance), and not plan L itself when a row there
//! covers j. Finding L by comparing every pair of rows is quadratic, so rows
//! are indexed by their constraint set instead: two rows are related only
//! when one constraint set is a subset of the other, and constraint sets
//! are few. A row whose constraints can't be hashed exactly (a repeated
//! field or role, a role referenced by IID in one row and by key in
//! another, a field with two value types) is compared pairwise.
//!
//! Cost: O(rows x (distinct constraint sets + pairwise rows)) lookups, plus
//! one backfill of the existing rows per new constraint set.

use std::collections::HashMap;

use type_bridge_contract::id::{AttributeId, RoleId, TypeId};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalDouble, CanonicalValue, ValueTypeTag};

use super::super::reference::{Reference, ReferenceIdentity};
use super::{KeylessRow, covers};

/// Rows share a plan only with the same fields and player routes, in order.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Shape {
    fields: Vec<(OwnsFactId, ValueTypeTag)>,
    routes: Vec<(RoleId, Vec<TypeId>, Option<KeyRoute>)>,
}

/// The key attribute and value type of a key reference; `None` is an IID.
type KeyRoute = (AttributeId, ValueTypeTag);

pub(super) fn shape(row: &KeylessRow<'_>) -> Shape {
    Shape {
        fields: row
            .fields
            .iter()
            .map(|(field, value)| (field.clone(), value.value_type()))
            .collect(),
        routes: row
            .players
            .iter()
            .map(|(role, reference)| {
                (
                    role.clone(),
                    reference.candidates.to_vec(),
                    key_route(reference),
                )
            })
            .collect(),
    }
}

fn key_route(reference: &Reference<'_>) -> Option<KeyRoute> {
    match &reference.identity {
        ReferenceIdentity::Iid(_) => None,
        ReferenceIdentity::Key { attribute, value } => {
            Some(((*attribute).clone(), value.value_type()))
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Constraint {
    Field(OwnsFactId),
    Role(RoleId),
}

/// What a constraint compares: a field's value type, or a role's identity.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Kind {
    Field(ValueTypeTag),
    Player(Option<KeyRoute>),
}

/// A value under `may_equal`: equal keys exactly when values may be equal.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Value(CanonicalValue),
    Instant(i128),
    /// Durations have no semantic order, so any two may be equal.
    Duration,
    Iid(String),
}

fn key(value: &CanonicalValue) -> Key {
    match value {
        // Doubles are finite; only signed zeros differ in bits but not value.
        CanonicalValue::Double(double) if double.get() == 0.0 => Key::Value(
            CanonicalValue::Double(CanonicalDouble::new(0.0).expect("finite zero")),
        ),
        CanonicalValue::DateTimeTz(value) => Key::Instant(value.semantic_utc_nanoseconds()),
        CanonicalValue::Duration(_) => Key::Duration,
        // Decimals are stored in their unique normalized spelling.
        other => Key::Value(other.clone()),
    }
}

/// A row's constraints, sorted, or `None` when it must be compared pairwise.
type Indexed = Option<(Vec<Constraint>, Vec<Key>)>;

fn indexed_rows(rows: &[KeylessRow<'_>]) -> Vec<Indexed> {
    let mut kinds = HashMap::<Constraint, Option<Kind>>::new();
    let mut note = |constraint: Constraint, kind: Kind| {
        kinds
            .entry(constraint)
            .and_modify(|seen| {
                if seen.as_ref() != Some(&kind) {
                    *seen = None;
                }
            })
            .or_insert(Some(kind));
    };
    for row in rows {
        for (field, value) in row.fields {
            note(
                Constraint::Field(field.clone()),
                Kind::Field(value.value_type()),
            );
        }
        for (role, reference) in row.players {
            note(
                Constraint::Role(role.clone()),
                Kind::Player(key_route(reference)),
            );
        }
    }
    rows.iter()
        .map(|row| {
            let mut pairs = row
                .fields
                .iter()
                .map(|(field, value)| (Constraint::Field(field.clone()), key(value)))
                .chain(row.players.iter().map(|(role, reference)| {
                    let value = match &reference.identity {
                        ReferenceIdentity::Iid(iid) => Key::Iid(iid.to_ascii_lowercase()),
                        ReferenceIdentity::Key { value, .. } => key(value),
                    };
                    (Constraint::Role(role.clone()), value)
                }))
                .collect::<Vec<_>>();
            pairs.sort_by(|left, right| left.0.cmp(&right.0));
            let exact = pairs.windows(2).all(|pair| pair[0].0 != pair[1].0)
                && pairs
                    .iter()
                    .all(|(constraint, _)| kinds[constraint].is_some());
            exact.then(|| pairs.into_iter().unzip())
        })
        .collect()
}

/// One distinct constraint set and the sets it contains or is contained in.
struct ConstraintSet {
    constraints: Vec<Constraint>,
    subsets: Vec<usize>,
    supersets: Vec<usize>,
    rows: Vec<usize>,
}

fn is_subset(small: &[Constraint], large: &[Constraint]) -> bool {
    let mut large = large.iter();
    small
        .iter()
        .all(|wanted| large.by_ref().any(|candidate| candidate == wanted))
}

fn project(constraints: &[Constraint], keys: &[Key], onto: &[Constraint]) -> Vec<Key> {
    let mut pairs = constraints.iter().zip(keys);
    onto.iter()
        .map(|wanted| {
            pairs
                .by_ref()
                .find(|(constraint, _)| *constraint == wanted)
                .map(|(_, key)| key.clone())
                .expect("projection onto a subset")
        })
        .collect()
}

#[derive(Default)]
struct Index {
    sets: Vec<ConstraintSet>,
    by_constraints: HashMap<Vec<Constraint>, usize>,
    /// (set t, subset s): t's rows projected onto s -> their last plan.
    plans: HashMap<(usize, usize), HashMap<Vec<Key>, usize>>,
}

impl Index {
    fn register(
        &mut self,
        constraints: &[Constraint],
        indexed: &[Indexed],
        placed: &[usize],
    ) -> usize {
        if let Some(&set) = self.by_constraints.get(constraints) {
            return set;
        }
        let new = self.sets.len();
        self.sets.push(ConstraintSet {
            constraints: constraints.to_vec(),
            subsets: vec![new],
            supersets: vec![new],
            rows: Vec::new(),
        });
        self.by_constraints.insert(constraints.to_vec(), new);
        self.plans.insert((new, new), HashMap::new());
        for other in 0..new {
            if is_subset(&self.sets[other].constraints, constraints) {
                self.sets[new].subsets.push(other);
                self.sets[other].supersets.push(new);
                self.plans.insert((new, other), HashMap::new());
            } else if is_subset(constraints, &self.sets[other].constraints) {
                self.sets[other].subsets.push(new);
                self.sets[new].supersets.push(other);
                let mut plans = HashMap::new();
                for &row in &self.sets[other].rows {
                    let (row_constraints, keys) = indexed[row].as_ref().expect("indexed row");
                    let plan = plans
                        .entry(project(row_constraints, keys, constraints))
                        .or_insert(placed[row]);
                    *plan = (*plan).max(placed[row]);
                }
                self.plans.insert((other, new), plans);
            }
        }
        new
    }

    /// The last plans of earlier rows covering this row, and covered by it.
    fn related(&self, set: usize, keys: &[Key]) -> (Option<usize>, Option<usize>) {
        let constraints = &self.sets[set].constraints;
        let covering = self.sets[set]
            .supersets
            .iter()
            .filter_map(|&larger| self.plans[&(larger, set)].get(keys).copied())
            .max();
        let covered = self.sets[set]
            .subsets
            .iter()
            .filter_map(|&smaller| {
                let onto = &self.sets[smaller].constraints;
                self.plans[&(smaller, smaller)]
                    .get(&project(constraints, keys, onto))
                    .copied()
            })
            .max();
        (covering, covered)
    }

    fn insert(&mut self, set: usize, row: usize, keys: &[Key], plan: usize) {
        self.sets[set].rows.push(row);
        for &smaller in &self.sets[set].subsets {
            let key = project(
                &self.sets[set].constraints,
                keys,
                &self.sets[smaller].constraints,
            );
            let last = self
                .plans
                .get_mut(&(set, smaller))
                .expect("subset index")
                .entry(key)
                .or_insert(plan);
            *last = (*last).max(plan);
        }
    }
}

/// Place rows into plans, in call-row order; each plan lists its rows.
pub(super) fn place(rows: &[KeylessRow<'_>]) -> Vec<Vec<usize>> {
    let indexed = indexed_rows(rows);
    let mut index = Index::default();
    let mut pairwise = Vec::new();
    let mut placed = Vec::with_capacity(rows.len());
    let mut plans: Vec<Vec<usize>> = Vec::new();
    let mut by_shape = HashMap::<Shape, Vec<usize>>::new();
    for (row, current) in rows.iter().enumerate() {
        let set = indexed[row]
            .as_ref()
            .map(|(constraints, _)| index.register(constraints, &indexed, &placed));
        let (mut covering, mut covered) = match (set, &indexed[row]) {
            (Some(set), Some((_, keys))) => index.related(set, keys),
            _ => (None, None),
        };
        let earlier: Box<dyn Iterator<Item = usize>> = if set.is_some() {
            Box::new(pairwise.iter().copied())
        } else {
            Box::new(0..row)
        };
        for other in earlier {
            if covers(&rows[other], current) {
                covering = covering.max(Some(placed[other]));
            }
            if covers(current, &rows[other]) {
                covered = covered.max(Some(placed[other]));
            }
        }
        let start = match covering.max(covered) {
            // A row in the last related plan covers this one: it must see it.
            Some(last) if covering == Some(last) => last + 1,
            Some(last) => last,
            None => 0,
        };
        let candidates = by_shape.entry(shape(current)).or_default();
        let at = candidates.partition_point(|&plan| plan < start);
        let plan = if let Some(&plan) = candidates.get(at) {
            plan
        } else {
            plans.push(Vec::new());
            candidates.push(plans.len() - 1);
            plans.len() - 1
        };
        plans[plan].push(row);
        placed.push(plan);
        match (set, &indexed[row]) {
            (Some(set), Some((_, keys))) => index.insert(set, row, keys, plan),
            _ => pairwise.push(row),
        }
    }
    plans
}

/// The quadratic pair scan `place` replaces; the test oracle.
#[cfg(test)]
pub(super) fn place_by_pairs(rows: &[KeylessRow<'_>]) -> Vec<Vec<usize>> {
    let mut plans: Vec<Vec<usize>> = Vec::new();
    let mut placed = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let last = (0..index)
            .filter(|&earlier| covers(&rows[earlier], row) || covers(row, &rows[earlier]))
            .map(|earlier| placed[earlier])
            .max();
        let target = plans
            .iter()
            .enumerate()
            .skip(last.unwrap_or(0))
            .find(|(plan, members)| {
                shape(&rows[members[0]]) == shape(row)
                    && (Some(*plan) != last
                        || !members.iter().any(|&member| covers(&rows[member], row)))
            })
            .map(|(plan, _)| plan);
        let plan = target.unwrap_or_else(|| {
            plans.push(Vec::new());
            plans.len() - 1
        });
        plans[plan].push(index);
        placed.push(plan);
    }
    plans
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::temporal::CanonicalDuration;
    use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue};

    use super::super::super::reference::{Reference, ReferenceIdentity};
    use super::super::KeylessRow;
    use super::{place, place_by_pairs};

    fn field(name: &str) -> OwnsFactId {
        OwnsFactId::new(
            TypeId::new(TypeKind::Relation, "purchase").unwrap(),
            AttributeId::new(name).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn indexed_placement_matches_the_pair_scan() {
        let account = [TypeId::new(TypeKind::Entity, "account").unwrap()];
        let key = AttributeId::new("account-id").unwrap();
        let other_key = AttributeId::new("balance").unwrap();
        let buyer = RoleId::new("purchase", "buyer").unwrap();
        let item = RoleId::new("purchase", "item").unwrap();
        let iids = [
            "0x1e00000000000000000001",
            "0x1E00000000000000000001",
            "0x1e00000000000000000002",
        ];
        let (name, amount, ratio, wait) = (
            field("name"),
            field("amount"),
            field("ratio"),
            field("wait"),
        );
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        for case in 0..400 {
            // Most cases keep each role on one identity kind (indexed rows);
            // some mix kinds or repeat fields (pairwise rows).
            let mixed = case % 4 == 0;
            let count = 1 + next(24) as usize;
            let mut fields = Vec::new();
            let mut players = Vec::new();
            for _ in 0..count {
                let mut row = Vec::new();
                if next(3) > 0 {
                    row.push((
                        name.clone(),
                        CanonicalValue::String(
                            CanonicalString::new(["a", "b"][next(2) as usize]).unwrap(),
                        ),
                    ));
                }
                if next(2) == 0 {
                    row.push((amount.clone(), CanonicalValue::Long(next(2) as i64)));
                }
                if next(4) == 0 {
                    let zero = if next(2) == 0 { -0.0 } else { 0.0 };
                    row.push((
                        ratio.clone(),
                        CanonicalValue::Double(CanonicalDouble::new(zero).unwrap()),
                    ));
                }
                if next(5) == 0 {
                    let days = next(3);
                    row.push((
                        wait.clone(),
                        CanonicalValue::Duration(
                            CanonicalDuration::new(false, 0, days, 0, 0).unwrap(),
                        ),
                    ));
                }
                if mixed && next(6) == 0 {
                    row.push((amount.clone(), CanonicalValue::Long(1)));
                }
                fields.push(row);
                let mut row_players = Vec::new();
                if next(2) == 0 {
                    let identity = if mixed && next(2) == 0 {
                        if next(2) == 0 {
                            ReferenceIdentity::Key {
                                attribute: &key,
                                value: CanonicalValue::String(CanonicalString::new("A1").unwrap()),
                            }
                        } else {
                            ReferenceIdentity::Key {
                                attribute: &other_key,
                                value: CanonicalValue::Long(7),
                            }
                        }
                    } else {
                        ReferenceIdentity::Iid(iids[next(3) as usize])
                    };
                    row_players.push((
                        buyer.clone(),
                        Reference {
                            candidates: &account,
                            identity,
                        },
                    ));
                }
                if next(3) == 0 {
                    let value = ["A1", "A2"][next(2) as usize];
                    row_players.push((
                        item.clone(),
                        Reference {
                            candidates: &account,
                            identity: ReferenceIdentity::Key {
                                attribute: &key,
                                value: CanonicalValue::String(CanonicalString::new(value).unwrap()),
                            },
                        },
                    ));
                }
                players.push(row_players);
            }
            let rows = fields
                .iter()
                .zip(&players)
                .enumerate()
                .map(|(ordinal, (fields, players))| KeylessRow {
                    ordinal,
                    fields,
                    players,
                })
                .collect::<Vec<_>>();
            assert_eq!(place(&rows), place_by_pairs(&rows), "case {case}");
        }
    }

    #[test]
    fn distinct_rows_of_one_shape_place_into_one_plan() {
        let name = field("name");
        let fields = (0..100_000)
            .map(|row| vec![(name.clone(), CanonicalValue::Long(row))])
            .collect::<Vec<_>>();
        let rows = fields
            .iter()
            .enumerate()
            .map(|(ordinal, fields)| KeylessRow {
                ordinal,
                fields,
                players: &[],
            })
            .collect::<Vec<_>>();
        let plans = place(&rows);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].len(), 100_000);
    }
}
