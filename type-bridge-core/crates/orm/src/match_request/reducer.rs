//! Canonical binding-neutral typed reduction execution.

use std::collections::{BTreeMap, BTreeSet};

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::BindingId;
use super::lowering::{LoweredReduceGroup, LoweredReduceInput, LoweredReduceTerm, ReduceDomain};
use super::model::Reduction;
use super::result::{
    BoundConceptEvidence, HydratedThing, ProviderResultEvidence, ProviderSolutionEvidence,
    ReducedValue, ReductionRow,
};
use super::validation::ValidatedMatchRequest;
use crate::error::OrmError;
use crate::value::AttributeValue;

fn decode_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResultDecode, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn resource_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

#[derive(Debug, Clone, PartialEq)]
enum CollectedInputs {
    Long(Vec<i64>),
    Double(Vec<f64>),
}

impl CollectedInputs {
    fn new(domain: ReduceDomain) -> Self {
        match domain {
            ReduceDomain::Long => Self::Long(Vec::new()),
            ReduceDomain::Double => Self::Double(Vec::new()),
        }
    }

    fn push(&mut self, value: &AttributeValue) -> Result<(), OrmError> {
        match (self, value) {
            (Self::Long(values), AttributeValue::Long(value)) => {
                values.push(*value);
                Ok(())
            }
            (Self::Double(values), AttributeValue::Double(value)) => {
                if !value.is_finite() {
                    return Err(decode_error(
                        "reduction_input_not_finite",
                        "provider reducer input is not a finite double",
                    ));
                }
                values.push(*value);
                Ok(())
            }
            _ => Err(decode_error(
                "reduction_input_domain",
                "provider reducer input does not match its validated domain",
            )),
        }
    }

    fn as_doubles(&self) -> Vec<f64> {
        match self {
            Self::Long(values) => values.iter().map(|value| *value as f64).collect(),
            Self::Double(values) => values.clone(),
        }
    }
}

fn finite_reduction_double(value: f64) -> Result<ReducedValue, OrmError> {
    if !value.is_finite() {
        return Err(resource_error(
            "reduction_overflow",
            "reduction result left the finite double domain",
        ));
    }
    Ok(ReducedValue::Double(Some(value)))
}

/// Reduce one collected input stream with canonical semantics: sums stay
/// total (zero on empty), extrema and statistical reducers are absent on
/// empty streams, sample standard deviation requires two values, and all
/// double results must remain finite.
fn reduce_collected(
    reduction: Reduction,
    collected: &CollectedInputs,
) -> Result<ReducedValue, OrmError> {
    match reduction {
        Reduction::Count => Err(decode_error(
            "reduction_input_domain",
            "count consumes the distinct root stream, not a field input",
        )),
        Reduction::Sum => match collected {
            CollectedInputs::Long(values) => {
                let mut total = 0_i64;
                for value in values {
                    total = total.checked_add(*value).ok_or_else(|| {
                        resource_error(
                            "reduction_overflow",
                            "integer sum left the canonical long domain",
                        )
                    })?;
                }
                Ok(ReducedValue::Long(Some(total)))
            }
            CollectedInputs::Double(values) => finite_reduction_double(values.iter().sum::<f64>()),
        },
        Reduction::Min | Reduction::Max => match collected {
            CollectedInputs::Long(values) => {
                let extreme = if reduction == Reduction::Min {
                    values.iter().min()
                } else {
                    values.iter().max()
                };
                Ok(ReducedValue::Long(extreme.copied()))
            }
            CollectedInputs::Double(values) => {
                let mut extreme: Option<f64> = None;
                for value in values {
                    extreme = Some(match extreme {
                        None => *value,
                        Some(current) if reduction == Reduction::Min => current.min(*value),
                        Some(current) => current.max(*value),
                    });
                }
                Ok(ReducedValue::Double(extreme))
            }
        },
        Reduction::Mean => {
            let values = collected.as_doubles();
            if values.is_empty() {
                return Ok(ReducedValue::Double(None));
            }
            finite_reduction_double(values.iter().sum::<f64>() / values.len() as f64)
        }
        Reduction::Median => {
            let mut values = collected.as_doubles();
            if values.is_empty() {
                return Ok(ReducedValue::Double(None));
            }
            values
                .sort_by(|left, right| left.partial_cmp(right).expect("reducer inputs are finite"));
            let middle = values.len() / 2;
            let median = if values.len() % 2 == 1 {
                values[middle]
            } else {
                (values[middle - 1] + values[middle]) / 2.0
            };
            finite_reduction_double(median)
        }
        Reduction::Std => {
            let values = collected.as_doubles();
            if values.len() < 2 {
                return Ok(ReducedValue::Double(None));
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance = values
                .iter()
                .map(|value| {
                    let delta = value - mean;
                    delta * delta
                })
                .sum::<f64>()
                / (values.len() - 1) as f64;
            finite_reduction_double(variance.sqrt())
        }
    }
}

fn reduction_input_value<'a>(
    solution: &'a ProviderSolutionEvidence,
    input: &LoweredReduceInput,
) -> Result<Option<&'a AttributeValue>, OrmError> {
    let thing = solution
        .bindings()
        .iter()
        .find(|bound| bound.binding() == input.binding)
        .map(BoundConceptEvidence::thing)
        .ok_or_else(|| {
            decode_error(
                "reduction_binding_missing",
                "provider solution omitted a reducer input binding",
            )
        })?;
    Ok(thing
        .attributes()
        .iter()
        .find(|attribute| attribute.field() == &input.field)
        .and_then(|attribute| attribute.values().first()))
}

pub(crate) fn reduction_evidence(
    validated: &ValidatedMatchRequest,
    root: BindingId,
    group: Option<LoweredReduceGroup>,
    rows: Vec<ReductionRow>,
) -> ProviderResultEvidence {
    match group {
        None => ProviderResultEvidence::reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            None,
            rows,
        ),
        Some(LoweredReduceGroup::Binding(group)) => ProviderResultEvidence::reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            Some(group),
            rows,
        ),
        Some(LoweredReduceGroup::Field(group)) => ProviderResultEvidence::field_reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            group,
            rows,
        ),
        Some(LoweredReduceGroup::Fields(groups)) => ProviderResultEvidence::field_tuple_reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            groups,
            rows,
        ),
    }
}

#[derive(Debug, Clone)]
struct AttributeGroupKey(AttributeValue);

impl PartialEq for AttributeGroupKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for AttributeGroupKey {}

impl PartialOrd for AttributeGroupKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AttributeGroupKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        attribute_domain_rank(&self.0)
            .cmp(&attribute_domain_rank(&other.0))
            .then_with(|| {
                super::result_validation::value_order(&self.0, &other.0)
                    .expect("canonical same-domain reduction keys are comparable")
            })
    }
}

const fn attribute_domain_rank(value: &AttributeValue) -> u8 {
    match value {
        AttributeValue::String(_) => 0,
        AttributeValue::Long(_) => 1,
        AttributeValue::Double(_) => 2,
        AttributeValue::Boolean(_) => 3,
        AttributeValue::Date(_) => 4,
        AttributeValue::DateTime(_) => 5,
        AttributeValue::DateTimeTZ(_) => 6,
        AttributeValue::Decimal(_) => 7,
        AttributeValue::Duration(_) => 8,
    }
}

fn attribute_group_key(value: &AttributeValue) -> Result<AttributeGroupKey, OrmError> {
    if super::result_validation::value_order(value, value).is_none() {
        return Err(decode_error(
            "reduction_group_value_invalid",
            "field-grouped reduction received a non-canonical scalar",
        ));
    }
    Ok(AttributeGroupKey(match value {
        AttributeValue::Double(value) if *value == 0.0 => AttributeValue::Double(0.0),
        value => value.clone(),
    }))
}

/// Assemble typed reduction rows over the distinct selected-identity stream.
///
/// The exhaustively proven root scan is authoritative for distinct root
/// counting; solutions contribute each distinct (group, root) pair's input
/// values exactly once, and grouped rows are deterministically ordered by
/// group concept identity.
pub(crate) fn reduce_rows(
    root: BindingId,
    group: Option<&LoweredReduceGroup>,
    terms: &[LoweredReduceTerm],
    roots: &[String],
    solutions: &[ProviderSolutionEvidence],
    max_group_rows: usize,
    max_collection_members: usize,
) -> Result<Vec<ReductionRow>, OrmError> {
    struct GroupAccumulator {
        thing: HydratedThing,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    struct FieldGroupAccumulator {
        value: AttributeValue,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    struct FieldTupleGroupAccumulator {
        values: Vec<AttributeValue>,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    let fresh_inputs = |terms: &[LoweredReduceTerm]| {
        terms
            .iter()
            .map(|term| {
                term.input
                    .as_ref()
                    .map(|input| CollectedInputs::new(input.domain))
            })
            .collect::<Vec<_>>()
    };
    let collect_solution = |accumulated: &mut Vec<Option<CollectedInputs>>,
                            solution: &ProviderSolutionEvidence|
     -> Result<(), OrmError> {
        for (term, collected) in terms.iter().zip(accumulated.iter_mut()) {
            let (Some(input), Some(collected)) = (&term.input, collected) else {
                continue;
            };
            if let Some(value) = reduction_input_value(solution, input)? {
                collected.push(value)?;
            }
        }
        Ok(())
    };
    let finish = |root_count: usize,
                  accumulated: &[Option<CollectedInputs>]|
     -> Result<Vec<ReducedValue>, OrmError> {
        terms
            .iter()
            .zip(accumulated)
            .map(|(term, collected)| match collected {
                None => Ok(ReducedValue::Count(root_count as u64)),
                Some(collected) => reduce_collected(term.reduction, collected),
            })
            .collect()
    };
    let solution_root = |solution: &ProviderSolutionEvidence| -> Result<String, OrmError> {
        solution
            .bindings()
            .iter()
            .find(|bound| bound.binding() == root)
            .map(|bound| bound.thing().concept_id().as_str().to_owned())
            .ok_or_else(|| {
                decode_error(
                    "reduction_binding_missing",
                    "provider solution omitted the reduced root binding",
                )
            })
    };
    let group_cells = match group {
        None => 0,
        Some(LoweredReduceGroup::Binding(_) | LoweredReduceGroup::Field(_)) => 1,
        Some(LoweredReduceGroup::Fields(fields)) => fields.len(),
    };
    let cells_per_row = group_cells.checked_add(terms.len()).ok_or_else(|| {
        resource_error(
            "collected_concept_limit",
            "reduction result cell width overflowed",
        )
    })?;
    let admit_group = |current: usize, message: &'static str| -> Result<(), OrmError> {
        if current >= max_group_rows {
            return Err(resource_error("reduction_group_limit", message));
        }
        let cells = current
            .checked_add(1)
            .and_then(|rows| rows.checked_mul(cells_per_row))
            .ok_or_else(|| {
                resource_error(
                    "collected_concept_limit",
                    "reduction result cell count overflowed",
                )
            })?;
        if cells > max_collection_members {
            return Err(resource_error(
                "collected_concept_limit",
                "reduction result exceeded the collection-member ceiling",
            ));
        }
        Ok(())
    };
    match group {
        None => {
            admit_group(0, "ungrouped reduction exceeded the result-row ceiling")?;
            let mut seen = BTreeSet::new();
            let mut accumulated = fresh_inputs(terms);
            for solution in solutions {
                let root_id = solution_root(solution)?;
                if seen.insert(root_id) {
                    collect_solution(&mut accumulated, solution)?;
                }
            }
            let values = finish(roots.len(), &accumulated)?;
            Ok(vec![ReductionRow::new(None, values)])
        }
        Some(LoweredReduceGroup::Binding(group)) => {
            let mut groups: BTreeMap<String, GroupAccumulator> = BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let thing = solution
                    .bindings()
                    .iter()
                    .find(|bound| bound.binding() == *group)
                    .map(BoundConceptEvidence::thing)
                    .ok_or_else(|| {
                        decode_error(
                            "reduction_binding_missing",
                            "provider solution omitted the group binding",
                        )
                    })?;
                let key = thing.concept_id().as_str().to_owned();
                if !groups.contains_key(&key) {
                    admit_group(
                        groups.len(),
                        "binding-grouped reduction exceeded the result-row ceiling",
                    )?;
                }
                let entry = groups.entry(key).or_insert_with(|| GroupAccumulator {
                    thing: thing.clone(),
                    roots: BTreeSet::new(),
                    inputs: fresh_inputs(terms),
                });
                if entry.roots.insert(root_id) {
                    collect_solution(&mut entry.inputs, solution)?;
                }
            }
            groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new(Some(accumulator.thing), values))
                })
                .collect()
        }
        Some(LoweredReduceGroup::Field(group)) => {
            let mut groups: BTreeMap<AttributeGroupKey, FieldGroupAccumulator> = BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let thing = solution
                    .bindings()
                    .iter()
                    .find(|bound| bound.binding() == group.binding)
                    .map(BoundConceptEvidence::thing)
                    .ok_or_else(|| {
                        decode_error(
                            "reduction_binding_missing",
                            "provider solution omitted the field-group owner binding",
                        )
                    })?;
                let Some(attribute) = thing
                    .attributes()
                    .iter()
                    .find(|attribute| attribute.field() == &group.field)
                else {
                    continue;
                };
                for value in attribute.values() {
                    let key = attribute_group_key(value)?;
                    if !groups.contains_key(&key) {
                        admit_group(
                            groups.len(),
                            "field-grouped reduction exceeded the result-row ceiling",
                        )?;
                    }
                    let entry = groups.entry(key).or_insert_with(|| FieldGroupAccumulator {
                        value: match value {
                            AttributeValue::Double(value) if *value == 0.0 => {
                                AttributeValue::Double(0.0)
                            }
                            value => value.clone(),
                        },
                        roots: BTreeSet::new(),
                        inputs: fresh_inputs(terms),
                    });
                    if entry.roots.insert(root_id.clone()) {
                        collect_solution(&mut entry.inputs, solution)?;
                    }
                }
            }
            let rows = groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new_field(accumulator.value, values))
                })
                .collect::<Result<Vec<_>, OrmError>>()?;
            Ok(rows)
        }
        Some(LoweredReduceGroup::Fields(group_fields)) => {
            let mut groups: BTreeMap<Vec<AttributeGroupKey>, FieldTupleGroupAccumulator> =
                BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let mut choices = Vec::with_capacity(group_fields.len());
                let mut omitted = false;
                for group in group_fields {
                    let thing = solution
                        .bindings()
                        .iter()
                        .find(|bound| bound.binding() == group.binding)
                        .map(BoundConceptEvidence::thing)
                        .ok_or_else(|| {
                            decode_error(
                                "reduction_binding_missing",
                                "provider solution omitted a tuple-group field owner binding",
                            )
                        })?;
                    let Some(attribute) = thing
                        .attributes()
                        .iter()
                        .find(|attribute| attribute.field() == &group.field)
                    else {
                        omitted = true;
                        break;
                    };
                    let mut distinct = BTreeMap::new();
                    for value in attribute.values() {
                        let key = attribute_group_key(value)?;
                        distinct.entry(key.clone()).or_insert(key.0);
                    }
                    if distinct.is_empty() {
                        omitted = true;
                        break;
                    }
                    choices.push(distinct.into_iter().collect::<Vec<_>>());
                }
                if omitted {
                    continue;
                }
                let tuple_count = choices.iter().try_fold(1_usize, |count, values| {
                    count.checked_mul(values.len()).ok_or_else(|| {
                        resource_error(
                            "reduction_group_limit",
                            "tuple-field reduction group cardinality overflowed",
                        )
                    })
                })?;
                if tuple_count > max_group_rows {
                    return Err(resource_error(
                        "reduction_group_limit",
                        "tuple-field reduction exceeded the result-row ceiling",
                    ));
                }
                let tuple_cells = tuple_count.checked_mul(cells_per_row).ok_or_else(|| {
                    resource_error(
                        "collected_concept_limit",
                        "tuple-field reduction result cell count overflowed",
                    )
                })?;
                if tuple_cells > max_collection_members {
                    return Err(resource_error(
                        "collected_concept_limit",
                        "tuple-field reduction exceeded the collection-member ceiling",
                    ));
                }
                let mut tuples = vec![(Vec::new(), Vec::new())];
                for values in choices {
                    let capacity = tuples.len().checked_mul(values.len()).ok_or_else(|| {
                        resource_error(
                            "reduction_group_limit",
                            "tuple-field reduction group cardinality overflowed",
                        )
                    })?;
                    let mut expanded = Vec::with_capacity(capacity);
                    for (keys, scalars) in &tuples {
                        for (key, scalar) in &values {
                            let mut next_keys = keys.clone();
                            next_keys.push(key.clone());
                            let mut next_scalars = scalars.clone();
                            next_scalars.push(scalar.clone());
                            expanded.push((next_keys, next_scalars));
                        }
                    }
                    tuples = expanded;
                }
                for (key, values) in tuples {
                    if !groups.contains_key(&key) {
                        admit_group(
                            groups.len(),
                            "tuple-field reduction exceeded the result-row ceiling",
                        )?;
                    }
                    let entry = groups
                        .entry(key)
                        .or_insert_with(|| FieldTupleGroupAccumulator {
                            values,
                            roots: BTreeSet::new(),
                            inputs: fresh_inputs(terms),
                        });
                    if entry.roots.insert(root_id.clone()) {
                        collect_solution(&mut entry.inputs, solution)?;
                    }
                }
            }
            let rows = groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new_fields(accumulator.values, values))
                })
                .collect::<Result<Vec<_>, OrmError>>()?;
            Ok(rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_request::ids::{BoundFieldId, DescriptorId, FieldId};
    use crate::match_request::model::ThingKind;
    use crate::match_request::result::{BoundConceptEvidence, ConceptId, HydratedAttribute};

    fn descriptor() -> DescriptorId {
        DescriptorId::new("entity:person")
    }

    fn field(name: &str) -> FieldId {
        FieldId::new(descriptor(), name)
    }

    fn thing(iid: &str, attributes: Vec<(&str, Vec<AttributeValue>)>) -> HydratedThing {
        HydratedThing::new(
            ConceptId::new(iid),
            descriptor(),
            descriptor(),
            ThingKind::Entity,
            attributes
                .into_iter()
                .map(|(name, values)| HydratedAttribute::new(field(name), values))
                .collect(),
            Vec::new(),
        )
    }

    fn solution(root_iid: &str, bindings: Vec<(u16, HydratedThing)>) -> ProviderSolutionEvidence {
        let mut evidence = vec![BoundConceptEvidence::new(
            BindingId::new(0),
            thing(root_iid, Vec::new()),
        )];
        evidence.extend(
            bindings
                .into_iter()
                .map(|(binding, thing)| BoundConceptEvidence::new(BindingId::new(binding), thing)),
        );
        ProviderSolutionEvidence::new(evidence, Vec::new())
    }

    fn count_term() -> LoweredReduceTerm {
        LoweredReduceTerm {
            reduction: Reduction::Count,
            input: None,
        }
    }

    fn numeric_term(
        reduction: Reduction,
        binding: u16,
        name: &str,
        domain: ReduceDomain,
    ) -> LoweredReduceTerm {
        LoweredReduceTerm {
            reduction,
            input: Some(LoweredReduceInput {
                binding: BindingId::new(binding),
                field: field(name),
                domain,
            }),
        }
    }

    fn match_code(error: OrmError) -> String {
        match error {
            OrmError::Match(error) => error.code().as_str().to_owned(),
            error => panic!("expected match error, got {error:?}"),
        }
    }

    #[test]
    fn binding_group_limit_accepts_exact_boundary_and_rejects_next_group() {
        let solutions = vec![
            solution("0x01", vec![(1, thing("0x10", Vec::new()))]),
            solution("0x02", vec![(1, thing("0x11", Vec::new()))]),
        ];
        let roots = vec!["0x01".to_owned(), "0x02".to_owned()];
        let group = LoweredReduceGroup::Binding(BindingId::new(1));

        let rows = reduce_rows(
            BindingId::new(0),
            Some(&group),
            &[count_term()],
            &roots,
            &solutions,
            2,
            4,
        )
        .expect("exact group-row boundary");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .map(|row| row.group().expect("thing group").concept_id().as_str())
                .collect::<Vec<_>>(),
            vec!["0x10", "0x11"]
        );

        let error = reduce_rows(
            BindingId::new(0),
            Some(&group),
            &[count_term()],
            &roots,
            &solutions,
            1,
            4,
        )
        .expect_err("one group beyond the ceiling");
        assert_eq!(match_code(error), "reduction_group_limit");
    }

    #[test]
    fn field_groups_use_semantic_numeric_order_and_canonical_positive_zero() {
        let group_field = BoundFieldId::new(BindingId::new(0), field("score"));
        let solutions = vec![
            solution(
                "0x01",
                vec![(
                    2,
                    thing(
                        "0x21",
                        vec![(
                            "score",
                            vec![AttributeValue::Double(-0.0), AttributeValue::Double(-1.0)],
                        )],
                    ),
                )],
            ),
            solution(
                "0x02",
                vec![(
                    2,
                    thing(
                        "0x22",
                        vec![(
                            "score",
                            vec![AttributeValue::Double(0.0), AttributeValue::Double(-2.0)],
                        )],
                    ),
                )],
            ),
        ];
        // Group binding 2 owns the tested field while binding 0 remains the
        // distinct reduced root.
        let group =
            LoweredReduceGroup::Field(BoundFieldId::new(BindingId::new(2), group_field.field));
        let roots = vec!["0x01".to_owned(), "0x02".to_owned()];
        let rows = reduce_rows(
            BindingId::new(0),
            Some(&group),
            &[count_term()],
            &roots,
            &solutions,
            3,
            6,
        )
        .expect("semantic double groups");
        let values = rows
            .iter()
            .map(|row| match row.field_group().expect("field group") {
                AttributeValue::Double(value) => *value,
                value => panic!("unexpected group value {value:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(values, vec![-2.0, -1.0, 0.0]);
        assert_eq!(values[2].to_bits(), 0.0_f64.to_bits());

        let mut reversed = solutions;
        reversed.reverse();
        let rows = reduce_rows(
            BindingId::new(0),
            Some(&group),
            &[count_term()],
            &roots,
            &reversed,
            3,
            6,
        )
        .expect("provider-order-independent zero group");
        let zero = rows[2].field_group().expect("zero group");
        assert!(matches!(zero, AttributeValue::Double(value) if value.to_bits() == 0));
    }

    #[test]
    fn decimal_and_tuple_groups_use_canonical_semantic_order() {
        let decimal_group =
            LoweredReduceGroup::Field(BoundFieldId::new(BindingId::new(2), field("balance")));
        let solutions = vec![solution(
            "0x01",
            vec![(
                2,
                thing(
                    "0x21",
                    vec![(
                        "balance",
                        vec![
                            AttributeValue::Decimal("2".into()),
                            AttributeValue::Decimal("-2".into()),
                            AttributeValue::Decimal("-10".into()),
                        ],
                    )],
                ),
            )],
        )];
        let roots = vec!["0x01".to_owned()];
        let rows = reduce_rows(
            BindingId::new(0),
            Some(&decimal_group),
            &[count_term()],
            &roots,
            &solutions,
            3,
            6,
        )
        .expect("semantic decimal groups");
        assert_eq!(
            rows.iter()
                .map(|row| row.field_group().expect("field group").clone())
                .collect::<Vec<_>>(),
            vec![
                AttributeValue::Decimal("-10".into()),
                AttributeValue::Decimal("-2".into()),
                AttributeValue::Decimal("2".into()),
            ]
        );

        let tuple_group = LoweredReduceGroup::Fields(vec![
            BoundFieldId::new(BindingId::new(2), field("balance")),
            BoundFieldId::new(BindingId::new(2), field("score")),
        ]);
        let tuple_solutions = vec![solution(
            "0x01",
            vec![(
                2,
                thing(
                    "0x21",
                    vec![
                        (
                            "balance",
                            vec![
                                AttributeValue::Decimal("2".into()),
                                AttributeValue::Decimal("-2".into()),
                            ],
                        ),
                        (
                            "score",
                            vec![AttributeValue::Double(-1.0), AttributeValue::Double(-2.0)],
                        ),
                    ],
                ),
            )],
        )];
        let rows = reduce_rows(
            BindingId::new(0),
            Some(&tuple_group),
            &[count_term()],
            &roots,
            &tuple_solutions,
            4,
            12,
        )
        .expect("semantic tuple groups");
        assert_eq!(
            rows.iter()
                .map(|row| row.field_groups().expect("tuple group").to_vec())
                .collect::<Vec<_>>(),
            vec![
                vec![
                    AttributeValue::Decimal("-2".into()),
                    AttributeValue::Double(-2.0),
                ],
                vec![
                    AttributeValue::Decimal("-2".into()),
                    AttributeValue::Double(-1.0),
                ],
                vec![
                    AttributeValue::Decimal("2".into()),
                    AttributeValue::Double(-2.0),
                ],
                vec![
                    AttributeValue::Decimal("2".into()),
                    AttributeValue::Double(-1.0),
                ],
            ]
        );
        let error = reduce_rows(
            BindingId::new(0),
            Some(&tuple_group),
            &[count_term()],
            &roots,
            &tuple_solutions,
            4,
            11,
        )
        .expect_err("one tuple group cell beyond the collection ceiling");
        assert_eq!(match_code(error), "collected_concept_limit");
    }

    #[test]
    fn ungrouped_reducers_are_total_on_empty_distinct_by_root_and_fail_closed_on_numbers() {
        let empty_terms = vec![
            count_term(),
            numeric_term(Reduction::Sum, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Min, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Max, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Mean, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Median, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Std, 2, "score", ReduceDomain::Long),
        ];
        let rows = reduce_rows(BindingId::new(0), None, &empty_terms, &[], &[], 1, 7)
            .expect("empty ungrouped reduction");
        assert_eq!(
            rows[0].values(),
            &[
                ReducedValue::Count(0),
                ReducedValue::Long(Some(0)),
                ReducedValue::Long(None),
                ReducedValue::Long(None),
                ReducedValue::Double(None),
                ReducedValue::Double(None),
                ReducedValue::Double(None),
            ]
        );

        let roots = vec!["0x01".to_owned(), "0x02".to_owned()];
        let repeated = vec![
            solution(
                "0x01",
                vec![(
                    2,
                    thing("0x21", vec![("score", vec![AttributeValue::Long(4)])]),
                )],
            ),
            solution(
                "0x01",
                vec![(
                    2,
                    thing("0x21", vec![("score", vec![AttributeValue::Long(4)])]),
                )],
            ),
            solution(
                "0x02",
                vec![(
                    2,
                    thing("0x22", vec![("score", vec![AttributeValue::Long(6)])]),
                )],
            ),
        ];
        let terms = vec![
            count_term(),
            numeric_term(Reduction::Sum, 2, "score", ReduceDomain::Long),
            numeric_term(Reduction::Mean, 2, "score", ReduceDomain::Long),
        ];
        let rows = reduce_rows(BindingId::new(0), None, &terms, &roots, &repeated, 1, 3)
            .expect("duplicate solution roots contribute once");
        assert_eq!(
            rows[0].values(),
            &[
                ReducedValue::Count(2),
                ReducedValue::Long(Some(10)),
                ReducedValue::Double(Some(5.0)),
            ]
        );
        assert_eq!(
            match rows[0].values()[2] {
                ReducedValue::Double(Some(value)) => value.to_bits(),
                _ => unreachable!(),
            },
            5.0_f64.to_bits()
        );

        let overflow = vec![
            solution(
                "0x01",
                vec![(
                    2,
                    thing(
                        "0x21",
                        vec![("score", vec![AttributeValue::Long(i64::MAX)])],
                    ),
                )],
            ),
            solution(
                "0x02",
                vec![(
                    2,
                    thing("0x22", vec![("score", vec![AttributeValue::Long(1)])]),
                )],
            ),
        ];
        let error = reduce_rows(
            BindingId::new(0),
            None,
            &[numeric_term(Reduction::Sum, 2, "score", ReduceDomain::Long)],
            &roots,
            &overflow,
            1,
            1,
        )
        .expect_err("integer overflow fails closed");
        assert_eq!(match_code(error), "reduction_overflow");

        let non_finite = vec![solution(
            "0x01",
            vec![(
                2,
                thing(
                    "0x21",
                    vec![("ratio", vec![AttributeValue::Double(f64::INFINITY)])],
                ),
            )],
        )];
        let error = reduce_rows(
            BindingId::new(0),
            None,
            &[numeric_term(
                Reduction::Mean,
                2,
                "ratio",
                ReduceDomain::Double,
            )],
            &roots[..1],
            &non_finite,
            1,
            1,
        )
        .expect_err("non-finite provider input fails closed");
        assert_eq!(match_code(error), "reduction_input_not_finite");
    }
}
