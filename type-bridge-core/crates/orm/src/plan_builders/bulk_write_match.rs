//! Full-match upserts whose predicates cannot fit one canonical plan.
use super::*;
use crate::plan_builders::reference::reference_binding;
use type_bridge_contract::migration_assertion::AssertionRolePlayer;

pub(crate) struct MatchProgram {
    probes: VecDeque<BuiltPlan>,
    candidates: Option<std::collections::BTreeSet<String>>,
    on_miss: Option<WriteProgram>,
    on_hit: Option<BuiltPlan>,
    selected: Option<WriteProgram>,
    authority: PlanAuthority,
    ordinal: usize,
}
impl MatchProgram {
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
            }));
        }
        if self.selected.is_none() {
            let candidates = self.candidates.take().ok_or_else(identity_error)?;
            self.selected = Some(if candidates.is_empty() {
                self.on_miss.take().ok_or_else(identity_error)?
            } else {
                // The ordinary one-row API refuses ambiguous full matches too.
                if candidates.len() != 1 {
                    return Err(identity_error());
                }
                let iid = candidates.into_iter().next().ok_or_else(identity_error)?;
                let plan = self.on_hit.take().ok_or_else(identity_error)?;
                WriteProgram::Compact(vec![BuiltPlan {
                    plan: plan.local().with_given_rows(
                        vec![InputRow::new(vec![Some(super::iid(&iid)?)])],
                        self.authority.declared(),
                    )?,
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
        for row in rows {
            let [RemoteValue::Thing { iid, .. }] = row.as_slice() else {
                return Err(identity_error());
            };
            step.check_identity(iid)?;
            matched.insert(iid.clone());
        }
        if let Some(candidates) = &mut self.candidates {
            candidates.retain(|iid| matched.contains(iid));
        } else {
            self.candidates = Some(matched);
        }
        Ok(())
    }
}

fn probe(
    authority: &PlanAuthority,
    shape: &WriteShape<'_>,
    row: &Row,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(ValueTransport::Given);
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
    if draft.bindings.len() + draft.inputs.len() > type_bridge_contract::limits::MAX_BINDINGS {
        return Err(invalid(
            "query_plan_binding_limit",
            "Match predicate variables exceed the canonical binding ceiling",
        ));
    }
    if patterns.len() > type_bridge_contract::limits::MAX_BOOLEAN_TERMS {
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
            .chunks(type_bridge_contract::limits::MAX_BOOLEAN_TERMS)
            .map(|chunk| {
                QueryPattern::Or {
                    branches: vec![chunk.to_vec()],
                }
                .into()
            })
            .collect();
    }
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
    let mut draft = PlanDraft::new(ValueTransport::Given);
    let root = draft.bind("root")?;
    let ordinal_binding = draft.bind("ordinal")?;
    let target = draft.value(super::iid("0x00")?)?;
    let on_hit = draft.finish(
        authority,
        vec![
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
        ],
        Operation::Rows,
        QueryOutput::Rows {
            columns: vec![ordinal_binding, root],
        },
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
