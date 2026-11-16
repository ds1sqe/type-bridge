//! One proxy batch proof, checked before any write through IID dependencies.
use super::*;
use crate::plan_builders::{ValueTransport, authority::PlanAuthority};
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::{
    Expr, InputRow, Operation, OrderDirection, OrderTerm, Pattern, QueryOperand, QueryOutput,
    QueryPattern, Stage,
};
use type_bridge_contract::query_remote::{StagedPlan, StagedResultRef};

pub(super) struct KeyGuard {
    pub(super) entries: Vec<StagedPlan>,
    pub(super) rows: Vec<(usize, String)>,
}

pub(super) fn build(
    authority: &PlanAuthority,
    model: &type_bridge_contract::projection::ModelProjection,
    rows: &[(usize, &ProjectedBatchRow)],
) -> std::result::Result<Option<KeyGuard>, SdkExecutionDiagnostic> {
    let mut checked = Vec::new();
    let mut fields = BTreeMap::new();
    for &(ordinal, row) in rows {
        let ProjectedBatchRow::Update { iid, replacement } = row else {
            continue;
        };
        let keys = replacement
            .fields()
            .iter()
            .filter_map(|(field, values)| {
                model
                    .query_tokens()
                    .fields()
                    .get(field)
                    .filter(|token| token.is_key())
                    .and_then(|token| {
                        values
                            .first()
                            .map(|value| (token.declaring_id().clone(), value.value().clone()))
                    })
            })
            .collect::<BTreeMap<_, _>>();
        if keys.is_empty() {
            continue;
        }
        for (field, value) in &keys {
            fields.insert(field.clone(), value.value_type());
        }
        checked.push((ordinal, iid.clone(), keys));
    }
    if checked.is_empty() {
        return Ok(None);
    }
    let build =
        || -> std::result::Result<Vec<StagedPlan>, type_bridge_contract::diagnostic::Diagnostic> {
            let mut draft = authority.draft(ValueTransport::Given);
            let ordinal = draft.bind("ordinal")?;
            let actual = draft.bind("actual")?;
            let matched = draft.bind("matched")?;
            let ordinal_input = draft.input(
                "ordinal",
                type_bridge_contract::value::ValueTypeTag::Long,
                false,
            )?;
            let iid_input = draft.input(
                "iid",
                type_bridge_contract::value::ValueTypeTag::String,
                false,
            )?;
            let isa_iid = |binding| {
                vec![
                    QueryPattern::Isa {
                        binding,
                        type_id: model.id().clone(),
                        include_subtypes: false,
                    },
                    QueryPattern::Iid {
                        binding,
                        iid: iid_input.clone(),
                    },
                ]
            };
            let mut matched_patterns = isa_iid(matched);
            for (index, (field, domain)) in fields.iter().enumerate() {
                let input = draft.input(&format!("key{index}"), *domain, false)?;
                let value = draft.bind("key_value")?;
                matched_patterns.extend([
                    QueryPattern::Has {
                        owner: matched,
                        attribute: value,
                        attribute_id: field.attribute().clone(),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: value },
                        right: input,
                    },
                ]);
            }
            let input_rows = checked.iter().map(|(index, iid, keys)| {
            let mut values = vec![Some(CanonicalValue::Long(*index as i64)), Some(CanonicalValue::String(type_bridge_contract::value::CanonicalString::new(iid.clone())?))];
            values.extend(fields.keys().map(|field| keys.get(field).cloned()));
            Ok(InputRow::new(values))
        }).collect::<std::result::Result<Vec<_>, type_bridge_contract::diagnostic::Diagnostic>>()?;
            let proof = draft.finish_rows(
                authority,
                vec![
                    Stage::Match {
                        patterns: vec![
                            Pattern::Let {
                                binding: ordinal,
                                expr: Expr::Operand(ordinal_input),
                            },
                            QueryPattern::Try {
                                patterns: isa_iid(actual),
                            }
                            .into(),
                            QueryPattern::Try {
                                patterns: matched_patterns,
                            }
                            .into(),
                        ],
                    },
                    Stage::Sort {
                        terms: vec![OrderTerm::new(ordinal, OrderDirection::Ascending)],
                    },
                ],
                Operation::Rows,
                QueryOutput::Rows {
                    columns: vec![ordinal, actual, matched],
                },
                Some(input_rows),
            )?;
            let proof = StagedPlan::new(
                proof.local(),
                proof.local().given_rows().unwrap().to_vec(),
                authority.declared(),
            )?;
            let mut gate = authority.draft(ValueTransport::Given);
            let input = gate.input(
                "key_guard_iid",
                type_bridge_contract::value::ValueTypeTag::String,
                false,
            )?;
            let value = gate.bind("guard")?;
            let gate = gate.finish_rows(
                authority,
                vec![Stage::Match {
                    patterns: vec![Pattern::Let {
                        binding: value,
                        expr: Expr::Operand(input),
                    }],
                }],
                Operation::Rows,
                QueryOutput::Rows {
                    columns: vec![value],
                },
                None,
            )?;
            let gate = StagedPlan::new_with_result_refs(
                gate.local(),
                vec![InputRow::new(vec![None])],
                vec![StagedResultRef {
                    input: type_bridge_contract::query_plan::InputColumnId::new(0),
                    source_plan: 0,
                    source_column: 2,
                    expected_types: vec![model.id().clone()],
                }],
                authority.declared(),
            )?;
            Ok(vec![proof, gate])
        };
    Ok(Some(KeyGuard {
        entries: build().map_err(crate::execution_diagnostic::lower_remote_query_diagnostic)?,
        rows: checked
            .into_iter()
            .map(|(ordinal, iid, _)| (ordinal, iid))
            .collect(),
    }))
}
