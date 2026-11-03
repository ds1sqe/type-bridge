//! Full-match upserts whose predicates cannot fit one canonical plan.
use super::*;
use crate::plan_builders::reference::reference_binding;
use type_bridge_contract::migration_assertion::AssertionRolePlayer;

pub(crate) struct MatchProgram {
    probes: VecDeque<BuiltPlan>,
    candidates: Option<Vec<String>>,
    on_miss: Option<WriteProgram>,
    on_hit: Option<BuiltPlan>,
    selected: Option<WriteProgram>,
    authority: PlanAuthority,
    ordinal: usize,
}
impl MatchProgram {
    pub(super) fn records_answers(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(WriteProgram::records_answers)
    }
    pub(super) fn record_answer(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        if let Some(selected) = &mut self.selected {
            selected.record_answer(step, answer)?;
        }
        Ok(())
    }
    pub(super) fn statement_count(&self) -> usize {
        self.probes.len()
            + self
                .on_miss
                .as_ref()
                .map_or(0, WriteProgram::statement_count)
                .max(1)
    }
    pub(super) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        if let Some(plan) = self.probes.pop_front() {
            return Ok(Some(WriteStep {
                plan,
                complete: false,
                ordinal: self.ordinal,
                expected_iid: None,
                missing_iid_noop: false,
                candidate_filter: true,
                reference_guard: false,
                reference_resolution: None,
            }));
        }
        if self.selected.is_none() {
            let candidates = self.candidates.take().ok_or_else(identity_error)?;
            self.selected = Some(if candidates.is_empty() {
                self.on_miss
                    .take()
                    .ok_or_else(identity_error)?
                    .prepare_static()?
            } else {
                let rows = candidates
                    .into_iter()
                    .map(|iid| Ok(InputRow::new(vec![Some(super::iid(&iid)?)])))
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                let plan = self.on_hit.take().ok_or_else(identity_error)?;
                WriteProgram::Compact(vec![BuiltPlan {
                    plan: plan
                        .local()
                        .with_given_rows(rows, self.authority.declared())?,
                    verified: plan.verified,
                }])
            });
        }
        self.selected.as_mut().ok_or_else(identity_error)?.next()
    }
    pub(super) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: Option<String>,
    ) -> Result<(), Diagnostic> {
        self.selected
            .as_mut()
            .ok_or_else(identity_error)?
            .accept_identity(step, iid)
    }
    pub(super) fn accept_candidates(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        let StagedAnswerV3::Rows { rows } = answer.to_staged_v3(step.plan.local())? else {
            return Err(identity_error());
        };
        let mut matched = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();
        for row in rows {
            let [RemoteValue::Thing { iid, .. }] = row.as_slice() else {
                return Err(identity_error());
            };
            step.check_identity(iid)?;
            if matched.insert(iid.clone()) {
                ordered.push(iid.clone());
            }
        }
        if let Some(candidates) = &mut self.candidates {
            candidates.retain(|iid| matched.contains(iid));
        } else {
            self.candidates = Some(ordered);
        }
        Ok(())
    }
}

fn group_reads(mut patterns: Vec<Pattern>, terms: usize) -> Result<Vec<Pattern>, Diagnostic> {
    if patterns.len() > terms {
        let reads = patterns
            .into_iter()
            .map(|pattern| match pattern {
                Pattern::Read(read) => Ok(read),
                Pattern::Iid { binding, iid } => Ok(QueryPattern::Iid {
                    binding,
                    iid: QueryOperand::Literal {
                        value: super::iid(&iid)?,
                    },
                }),
                _ => unreachable!("candidate filters contain only reads"),
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        patterns = reads
            .chunks(terms)
            .map(|chunk| {
                QueryPattern::Or {
                    branches: vec![chunk.to_vec()],
                }
                .into()
            })
            .collect();
    }
    Ok(patterns)
}

fn probe(
    authority: &PlanAuthority,
    shape: &WriteShape<'_>,
    row: &Row,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(ValueTransport::Given);
    let root = draft.bind("root")?;
    let mut patterns = vec![
        QueryPattern::Isa {
            binding: root,
            type_id: shape.type_id.clone(),
            include_subtypes: true,
        }
        .into(),
    ];
    for (field, value) in &row.fields {
        let attribute = draft.bind("field")?;
        let value = draft.value(value.clone())?;
        patterns.extend([
            QueryPattern::Has {
                attribute,
                owner: root,
                attribute_id: field.attribute().clone(),
            }
            .into(),
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding { binding: attribute },
                right: value,
            }
            .into(),
        ]);
    }
    for player in &row.players {
        let identity = if let Some(iid) = &player.iid {
            ReferenceIdentity::Iid(iid)
        } else {
            let (attribute, value) = player.key.as_ref().expect("owned reference identity");
            ReferenceIdentity::Key {
                attribute,
                value: value.clone(),
            }
        };
        let binding = reference_binding(
            &mut draft,
            &Reference {
                candidates: &player.candidates,
                identity,
            },
            &mut patterns,
        )?;
        patterns.push(
            QueryPattern::Links {
                players: vec![AssertionRolePlayer::new(player.role.clone(), binding)],
                relation: root,
                relation_id: shape.type_id.clone(),
            }
            .into(),
        );
    }
    // Given columns and concept bindings both become provider variables.
    // Apply the same canonical binding ceiling to their combined width.
    if draft.bindings.len() + draft.inputs.len() > authority.query_limits().structural.bindings {
        return Err(invalid(
            "query_plan_binding_limit",
            "Match predicate variables exceed the canonical binding ceiling",
        ));
    }
    let terms = authority.query_limits().structural.boolean_terms;
    // Split wide scalar conjunctions before they become one expensive provider join.
    if row.fields.len().saturating_mul(2).saturating_add(1) > terms {
        return Err(invalid(
            "query_plan_pattern_limit",
            "Match scalar predicates exceed the configured Boolean-term ceiling",
        ));
    }
    let patterns = group_reads(patterns, terms)?;
    draft.finish(
        authority,
        vec![
            Stage::Match { patterns },
            Stage::Select {
                bindings: vec![root],
            },
            Stage::Distinct,
        ],
        Operation::Rows,
        QueryOutput::Rows {
            columns: vec![root],
        },
    )
}

pub(in crate::plan_builders::bulk_write) fn wide_match(
    authority: &PlanAuthority,
    shape: WriteShape<'_>,
    row: &WriteRow<'_>,
    ordinal: usize,
) -> Result<WriteProgram, Diagnostic> {
    if let Some(program) = static_scalar_match(authority, &shape, row, ordinal)? {
        return Ok(program);
    }
    let mut remaining = Row::of(row);
    let empty = Row {
        iid: None,
        fields: vec![],
        players: vec![],
    };
    let mut probes = VecDeque::new();
    while remaining.len() > 0 {
        let mut low = 0;
        let mut high = remaining.len();
        while low < high {
            let count = low + (high - low).div_ceil(2);
            match probe(authority, &shape, &Row::prefix(&empty, &remaining, count)) {
                Ok(_) => low = count,
                Err(error) if forces_pieces(&error) => high = count - 1,
                Err(error) => return Err(error),
            }
        }
        if low == 0 {
            return Err(invalid(
                "projected_batch_input_unit_byte_limit",
                "One match predicate exceeds the canonical write-piece ceiling",
            ));
        }
        probes.push_back(probe(
            authority,
            &shape,
            &Row::prefix(&empty, &remaining, low),
        )?);
        remaining.drain(low);
    }
    let mut draft = authority.draft(ValueTransport::Given);
    let root = draft.bind("root")?;
    let ordinal_binding = draft.bind("ordinal")?;
    let target = draft.value(super::iid("0x00")?)?;
    let mut pipeline = vec![
        Stage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: root,
                    type_id: shape.type_id.clone(),
                    include_subtypes: true,
                }
                .into(),
                QueryPattern::Iid {
                    binding: root,
                    iid: target,
                }
                .into(),
                Pattern::Let {
                    binding: ordinal_binding,
                    expr: Expr::Operand(QueryOperand::Literal {
                        value: CanonicalValue::Long(
                            i64::try_from(ordinal).map_err(|_| identity_error())?,
                        ),
                    }),
                },
            ],
        },
        Stage::Select {
            bindings: vec![root, ordinal_binding],
        },
        Stage::Distinct,
    ];
    let columns = if shape.return_documents {
        let mut document = ThingDocument::new(root, shape.document_roles.clone());
        document
            .scalars
            .push(type_bridge_contract::query_plan::v3::DocumentScalar {
                key: type_bridge_contract::migration_assertion::QueryVariable::new("ordinal")?,
                binding: ordinal_binding,
            });
        pipeline.push(Stage::Fetch(document));
        Vec::new()
    } else {
        vec![ordinal_binding, root]
    };
    let on_hit = draft.finish(
        authority,
        pipeline,
        Operation::Rows,
        QueryOutput::Rows { columns },
    )?;
    let on_miss = split_at(
        authority,
        WriteKind::Insert,
        shape,
        std::slice::from_ref(row),
        ordinal,
    )?;
    Ok(WriteProgram::MatchWide(Box::new(MatchProgram {
        probes,
        candidates: None,
        on_miss: Some(on_miss),
        on_hit: Some(on_hit),
        selected: None,
        authority: authority.clone(),
        ordinal,
    })))
}

fn scalar_probe(
    authority: &PlanAuthority,
    shape: &WriteShape<'_>,
    row: &WriteRow<'_>,
    ordinal: usize,
    complete: bool,
    single_field: bool,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(ValueTransport::Given);
    let root = draft.bind("root")?;
    let output_ordinal = draft.bind("ordinal")?;
    let field = draft.bind("field")?;
    let wanted = draft.bind("wanted")?;
    let count = draft.bind("matched")?;
    // Duplicate unordered predicates remain existential, including when two
    // distinct fields carry the same scalar value.
    let (patterns, rows) = if single_field {
        let input = draft.input("wanted_value", row.fields[0].1.value_type(), false)?;
        let values = row
            .fields
            .iter()
            .map(|(_, value)| {
                Ok((
                    type_bridge_contract::codec::to_canonical_json(value)?,
                    value.clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
        let rows = values
            .into_values()
            .map(|value| InputRow::new(vec![Some(value)]))
            .collect::<Vec<_>>();
        (
            vec![
                QueryPattern::Isa {
                    binding: root,
                    type_id: shape.type_id.clone(),
                    include_subtypes: true,
                }
                .into(),
                QueryPattern::Has {
                    owner: root,
                    attribute: field,
                    attribute_id: row.fields[0].0.attribute().clone(),
                }
                .into(),
                Pattern::Let {
                    binding: wanted,
                    expr: Expr::Operand(input),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: field },
                    right: QueryOperand::Binding { binding: wanted },
                }
                .into(),
            ],
            rows,
        )
    } else {
        let terms = authority.query_limits().structural.boolean_terms;
        if terms < 3 {
            return Err(invalid(
                "query_plan_pattern_limit",
                "Mixed scalar predicates require three Boolean terms",
            ));
        }
        let input = draft.input("wanted_predicate", ValueTypeTag::Long, false)?;
        let values = row
            .fields
            .iter()
            .map(|(field, value)| {
                Ok((
                    (
                        field.clone(),
                        type_bridge_contract::codec::to_canonical_json(value)?,
                    ),
                    value.clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
        let mut branches = Vec::with_capacity(values.len());
        let mut rows = Vec::with_capacity(values.len());
        for (index, ((owner, _), value)) in values.into_iter().enumerate() {
            let ordinal = CanonicalValue::Long(i64::try_from(index).map_err(|_| identity_error())?);
            rows.push(InputRow::new(vec![Some(ordinal.clone())]));
            branches.push(vec![
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: wanted },
                    right: QueryOperand::Literal { value: ordinal },
                },
                QueryPattern::Has {
                    owner: root,
                    attribute: field,
                    attribute_id: owner.attribute().clone(),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: field },
                    right: QueryOperand::Literal { value },
                },
            ]);
        }
        while branches.len() > terms {
            branches = branches
                .chunks(terms)
                .map(|branches| {
                    vec![QueryPattern::Or {
                        branches: branches.to_vec(),
                    }]
                })
                .collect();
        }
        (
            vec![
                QueryPattern::Isa {
                    binding: root,
                    type_id: shape.type_id.clone(),
                    include_subtypes: true,
                }
                .into(),
                Pattern::Let {
                    binding: wanted,
                    expr: Expr::Operand(input),
                },
                QueryPattern::Or { branches }.into(),
            ],
            rows,
        )
    };
    let required = i64::try_from(rows.len()).map_err(|_| identity_error())?;
    let mut pipeline = vec![
        Stage::Match { patterns },
        Stage::Select {
            bindings: vec![root, wanted],
        },
        Stage::Distinct,
        Stage::Reduce {
            assignments: vec![ReduceAssignment::new(count, Reducer::Count, None)],
            groups: vec![root],
        },
        Stage::Match {
            patterns: vec![
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: count },
                    right: QueryOperand::Literal {
                        value: CanonicalValue::Long(required),
                    },
                }
                .into(),
                Pattern::Let {
                    binding: output_ordinal,
                    expr: Expr::Operand(QueryOperand::Literal {
                        value: CanonicalValue::Long(
                            i64::try_from(ordinal).map_err(|_| identity_error())?,
                        ),
                    }),
                },
            ],
        },
        Stage::Select {
            bindings: vec![root, output_ordinal],
        },
    ];
    let output = if complete && shape.return_documents {
        let mut document = ThingDocument::new(root, shape.document_roles.clone());
        document
            .scalars
            .push(type_bridge_contract::query_plan::v3::DocumentScalar {
                key: type_bridge_contract::migration_assertion::QueryVariable::new("ordinal")?,
                binding: output_ordinal,
            });
        pipeline.push(Stage::Fetch(document));
        QueryOutput::Rows { columns: vec![] }
    } else {
        QueryOutput::Rows {
            columns: vec![output_ordinal, root],
        }
    };
    draft.finish_rows(authority, pipeline, Operation::Rows, output, Some(rows))
}

fn static_scalar_match(
    authority: &PlanAuthority,
    shape: &WriteShape<'_>,
    row: &WriteRow<'_>,
    ordinal: usize,
) -> Result<Option<WriteProgram>, Diagnostic> {
    if row.fields.is_empty() {
        return Ok(None);
    }
    if shape.type_id.kind() != TypeKind::Entity || !row.players.is_empty() {
        return Ok(None);
    }
    let single_field = row
        .fields
        .iter()
        .all(|(field, _)| field == &row.fields[0].0);
    let probe = match scalar_probe(authority, shape, row, ordinal, false, single_field) {
        Ok(plan) => plan,
        Err(error) if forces_pieces(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let final_read = scalar_probe(authority, shape, row, ordinal, true, single_field)?;
    let mut miss_shape = shape.clone();
    miss_shape.return_documents = false;
    let WriteProgram::Split(miss) = split_at(
        authority,
        WriteKind::Insert,
        miss_shape,
        std::slice::from_ref(row),
        ordinal,
    )?
    else {
        return Err(identity_error());
    };
    Ok(Some(WriteProgram::Static(Box::new(
        static_stack::StaticProgram::with_match_probe(miss, probe, final_read, ordinal)?,
    ))))
}
