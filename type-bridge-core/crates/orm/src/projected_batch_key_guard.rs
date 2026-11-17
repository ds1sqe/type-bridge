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
        checked.push((ordinal, iid.clone(), keys));
    }
    build_checked(authority, model, checked)
}

fn build_checked(
    authority: &PlanAuthority,
    model: &type_bridge_contract::projection::ModelProjection,
    checked: Vec<(usize, String, BTreeMap<OwnsFactId, CanonicalValue>)>,
) -> std::result::Result<Option<KeyGuard>, SdkExecutionDiagnostic> {
    if checked.is_empty() {
        return Ok(None);
    }
    let build = |checked: &[(usize, String, BTreeMap<OwnsFactId, CanonicalValue>)],
                 source_plan: usize|
     -> std::result::Result<
        Vec<StagedPlan>,
        type_bridge_contract::diagnostic::Diagnostic,
    > {
        let fields = &checked[0].2;
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
        for (index, (field, sample)) in fields.iter().enumerate() {
            let input = draft.input(&format!("key{index}"), sample.value_type(), false)?;
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
        let input_rows = checked
            .iter()
            .map(|(index, iid, keys)| {
                let mut values = vec![
                    Some(CanonicalValue::Long(*index as i64)),
                    Some(CanonicalValue::String(
                        type_bridge_contract::value::CanonicalString::new(iid.clone())?,
                    )),
                ];
                values.extend(fields.keys().map(|field| keys.get(field).cloned()));
                Ok(InputRow::new(values))
            })
            .collect::<std::result::Result<Vec<_>, type_bridge_contract::diagnostic::Diagnostic>>(
            )?;
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
                source_plan: source_plan as u32,
                source_column: 2,
                expected_types: vec![model.id().clone()],
            }],
            authority.declared(),
        )?;
        Ok(vec![proof, gate])
    };
    let mut entries = Vec::new();
    let limit = authority
        .query_limits()
        .structural
        .input_rows
        .min(crate::plan_builders::bulk_write::INDEXED_IID_ROWS);
    let mut start = 0;
    while start < checked.len() {
        let mut end = start + 1;
        while end < checked.len()
            && end - start < limit
            && checked[end].2.keys().eq(checked[start].2.keys())
        {
            end += 1;
        }
        entries.extend(
            build(&checked[start..end], entries.len())
                .map_err(crate::execution_diagnostic::lower_remote_query_diagnostic)?,
        );
        start = end;
    }
    Ok(Some(KeyGuard {
        entries,
        rows: checked
            .into_iter()
            .map(|(ordinal, iid, _)| (ordinal, iid))
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_key_guards_allow_different_supplied_parts_in_each_row_and_chunk() {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let documents = SchemaDocumentSet::parse([(DocumentId::new("composite.yaml").unwrap(), "format: typebridge.schema/v2\nattributes:\n  first-key: { value: string }\n  second-key: { value: string }\nentities:\n  person:\n    owns:\n      first-key: { key: true }\n      second-key: { key: true }\n")]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let resolved = resolve(
            &declared,
            &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        let kind = TypeId::new(TypeKind::Entity, "person").unwrap();
        let authority = installed.plan_authority().unwrap();
        let rows = (0..130)
            .map(|index| ProjectedBatchRow::Update {
                iid: format!("0x{:x}", index + 16),
                replacement: crate::ProjectedCreate::try_new(
                    &installed,
                    kind.clone(),
                    ["first-key", "second-key"]
                        .into_iter()
                        .map(|name| {
                            (
                                OwnsFactId::new(
                                    kind.clone(),
                                    type_bridge_contract::id::AttributeId::new(name).unwrap(),
                                )
                                .unwrap(),
                                vec![
                                    crate::ProjectedAttributeValue::try_new(
                                        &installed,
                                        TypeId::new(TypeKind::Attribute, name).unwrap(),
                                        CanonicalValue::String(
                                            type_bridge_contract::value::CanonicalString::new(
                                                format!("{name}-{index}"),
                                            )
                                            .unwrap(),
                                        ),
                                    )
                                    .unwrap(),
                                ],
                            )
                        })
                        .collect(),
                    vec![],
                )
                .unwrap(),
            })
            .collect::<Vec<_>>();
        let checked = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let ProjectedBatchRow::Update { iid, replacement } = row else {
                    unreachable!()
                };
                let keys = replacement
                    .fields()
                    .iter()
                    .enumerate()
                    .filter(|(part, _)| (index / 65) % 2 == *part)
                    .map(|(_, (field, values))| (field.clone(), values[0].value().clone()))
                    .collect();
                (index, iid.clone(), keys)
            })
            .collect();
        let guard = build_checked(
            &authority,
            installed.projection().models().get(&kind).unwrap(),
            checked,
        )
        .unwrap()
        .unwrap();
        assert_eq!(guard.entries.len(), 8);
        for proof in guard.entries.iter().step_by(2) {
            assert_eq!(proof.plan().inputs().len(), 3);
            assert!(
                proof
                    .rows()
                    .iter()
                    .all(|row| row.values().iter().all(Option::is_some))
            );
        }
    }
}
