//! Static IID dependencies for forced allocation and attachment pieces.
use super::*;

#[cfg(all(test, feature = "typedb"))]
#[path = "bulk_write_proxy_live.rs"]
mod proxy_live;

struct StaticStep {
    complete: bool,
    ordinal: usize,
    missing_iid_noop: bool,
    expected_iid: Option<String>,
    reference_resolution: Option<Box<references::ResolutionStep>>,
}

pub(crate) struct StaticProgram {
    entries: Vec<type_bridge_contract::query_remote_v3::StagedPlanV3>,
    steps: VecDeque<StaticStep>,
    answers: Vec<StagedAnswerV3>,
    authority: PlanAuthority,
}
impl StaticProgram {
    pub(super) fn new(program: SplitProgram) -> Result<Self, Diagnostic> {
        use type_bridge_contract::query_plan::v3::QueryPlanV3;
        use type_bridge_contract::query_plan::{InputColumn, InputColumnId};
        use type_bridge_contract::query_remote_v3::{StagedPlanV3, StagedResultRefV3};
        let mut entries: Vec<StagedPlanV3> = Vec::with_capacity(program.steps.len());
        let mut steps = VecDeque::with_capacity(program.steps.len());
        let mut source = None;
        let mut guard_source = None;
        for step in program.steps {
            let (mut entry, metadata) = match step {
                Step::Ready(step) => {
                    if !step.complete && !step.reference_guard {
                        source = Some((entries.len(), step.ordinal, step.missing_iid_noop));
                    }
                    let entry = StagedPlanV3::new(
                        step.plan.local(),
                        step.plan.local().given_rows().unwrap_or_default().to_vec(),
                        program.authority.declared(),
                    )?;
                    (
                        entry,
                        StaticStep {
                            complete: step.complete,
                            ordinal: step.ordinal,
                            missing_iid_noop: step.missing_iid_noop,
                            expected_iid: step.expected_iid,
                            reference_resolution: step.reference_resolution,
                        },
                    )
                }
                Step::Target {
                    plan,
                    complete,
                    ordinal,
                } => {
                    let (index, source_ordinal, optional) = source.ok_or_else(identity_error)?;
                    if source_ordinal != ordinal {
                        return Err(identity_error());
                    }
                    let source_plan = entries[index].plan();
                    let QueryOutput::Rows { columns } = source_plan.output() else {
                        return Err(identity_error());
                    };
                    let expected_types = source_plan
                        .binding_types(columns[1])
                        .ok_or_else(identity_error)?
                        .iter()
                        .cloned()
                        .collect();
                    let plan = plan.local();
                    let column = plan
                        .inputs()
                        .iter()
                        .position(|input| input.public_name().as_str() == "target_iid")
                        .ok_or_else(identity_error)?;
                    let mut inputs = plan.inputs().to_vec();
                    let input = &inputs[column];
                    inputs[column] = InputColumn::new(
                        input.id(),
                        input.public_name().clone(),
                        input.value_type(),
                        optional,
                    );
                    let mut rows = plan.given_rows().ok_or_else(identity_error)?.to_vec();
                    for row in &mut rows {
                        let mut cells = row.values().to_vec();
                        cells[column] = None;
                        *row = InputRow::new(cells);
                    }
                    let unbound = QueryPlanV3::new_with_limits(
                        crate::plan_builders::PlanSpec {
                            bindings: plan.bindings().to_vec(),
                            inputs,
                            given_rows: None,
                            operation: plan.operation(),
                            pipeline: plan.pipeline().to_vec(),
                            output: plan.output().clone(),
                            managed_semantics: plan.managed_semantics().clone(),
                            compatibility: plan.compatibility().cloned(),
                            functions: plan.functions().to_vec(),
                        },
                        program.authority.declared(),
                        plan.limits(),
                    )?;
                    let entry = StagedPlanV3::new_with_result_refs(
                        &unbound,
                        rows,
                        vec![StagedResultRefV3 {
                            input: InputColumnId::new(
                                u16::try_from(column).map_err(|_| identity_error())?,
                            ),
                            source_plan: u32::try_from(index).map_err(|_| identity_error())?,
                            source_column: 1,
                            expected_types,
                        }],
                        program.authority.declared(),
                    )?;
                    (
                        entry,
                        StaticStep {
                            complete,
                            ordinal,
                            missing_iid_noop: optional,
                            expected_iid: None,
                            reference_resolution: None,
                        },
                    )
                }
            };
            if let Some(index) = guard_source.take() {
                entry = gate(entry, &entries[index], index, &program.authority)?;
            }
            if metadata.reference_resolution.is_some() {
                guard_source = Some(entries.len());
            }
            entries.push(entry);
            steps.push_back(metadata);
        }
        type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(
            &entries,
            program.authority.declared(),
        )?;
        Ok(Self {
            entries,
            steps,
            answers: Vec::new(),
            authority: program.authority,
        })
    }

    pub(super) fn with_reference_lookup(
        body: SplitProgram,
        lookups: Vec<(BuiltPlan, Vec<usize>)>,
        placeholders: BTreeMap<String, usize>,
    ) -> Result<Self, Diagnostic> {
        use type_bridge_contract::query_plan::InputColumnId;
        use type_bridge_contract::query_remote_v3::{StagedPlanV3, StagedResultRefV3};
        let mut program = Self::new(body)?;
        let mut entries = Vec::with_capacity(program.entries.len() + lookups.len());
        let mut sources = BTreeMap::new();
        let mut prefix = VecDeque::new();
        for (lookup, ordinals) in lookups {
            let mut entry = StagedPlanV3::new(
                lookup.local(),
                lookup.local().given_rows().unwrap_or_default().to_vec(),
                program.authority.declared(),
            )?;
            let index = entries.len();
            for (position, ordinal) in ordinals.iter().enumerate() {
                sources.insert(*ordinal, (index, position * 2 + 1));
            }
            if let Some(previous) = entries.last() {
                entry = gate(entry, previous, index - 1, &program.authority)?;
            }
            entries.push(entry);
            prefix.push_back(StaticStep {
                complete: false,
                ordinal: 0,
                missing_iid_noop: false,
                expected_iid: None,
                reference_resolution: Some(Box::new(references::ResolutionStep {
                    ordinals,
                    guard: None,
                })),
            });
        }
        let last_source = entries.len().checked_sub(1).ok_or_else(identity_error)?;
        let mut seen = std::collections::BTreeSet::new();
        for (index, entry) in program.entries.into_iter().enumerate() {
            let plan = entry.plan();
            let mut rows = entry
                .rows()
                .iter()
                .map(|row| row.values().to_vec())
                .collect::<Vec<_>>();
            let offset = u32::try_from(last_source + 1).map_err(|_| identity_error())?;
            let mut refs = entry
                .result_refs()
                .iter()
                .cloned()
                .map(|mut reference| {
                    reference.source_plan = reference
                        .source_plan
                        .checked_add(offset)
                        .ok_or_else(identity_error)?;
                    Ok(reference)
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            for (column, input) in plan.inputs().iter().enumerate() {
                if placeholders.is_empty() || !input.public_name().as_str().starts_with("player_") {
                    continue;
                }
                let mut ordinal = None;
                for row in &mut rows {
                    let Some(CanonicalValue::String(value)) = &row[column] else {
                        continue;
                    };
                    let mapped = *placeholders
                        .get(value.as_str())
                        .ok_or_else(identity_error)?;
                    if ordinal.is_some_and(|previous| previous != mapped) {
                        return Err(identity_error());
                    }
                    ordinal = Some(mapped);
                    row[column] = None;
                }
                if let Some(ordinal) = ordinal {
                    seen.insert(ordinal);
                    let &(source_index, source_column) =
                        sources.get(&ordinal).ok_or_else(identity_error)?;
                    let source = entries[source_index].plan();
                    let QueryOutput::Rows { columns } = source.output() else {
                        return Err(identity_error());
                    };
                    refs.push(StagedResultRefV3 {
                        input: InputColumnId::new(
                            u16::try_from(column).map_err(|_| identity_error())?,
                        ),
                        source_plan: u32::try_from(source_index).map_err(|_| identity_error())?,
                        source_column: u16::try_from(source_column)
                            .map_err(|_| identity_error())?,
                        expected_types: source
                            .binding_types(columns[source_column])
                            .ok_or_else(identity_error)?
                            .iter()
                            .cloned()
                            .collect(),
                    });
                }
            }
            let entry = StagedPlanV3::new_with_result_refs(
                plan,
                rows.into_iter().map(InputRow::new).collect(),
                refs,
                program.authority.declared(),
            )?;
            entries.push(if index == 0 {
                gate(
                    entry,
                    &entries[last_source],
                    last_source,
                    &program.authority,
                )?
            } else {
                entry
            });
        }
        if seen.len() != placeholders.len() {
            return Err(identity_error());
        }
        type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(
            &entries,
            program.authority.declared(),
        )?;
        prefix.append(&mut program.steps);
        program.steps = prefix;
        program.entries = entries;
        Ok(program)
    }

    pub(super) fn with_match_probe(
        miss: SplitProgram,
        guards: Vec<WriteStep>,
        probes: Vec<BuiltPlan>,
        final_reads: Vec<BuiltPlan>,
        ordinal: usize,
    ) -> Result<Self, Diagnostic> {
        use type_bridge_contract::migration_assertion::QueryVariable;
        use type_bridge_contract::query_plan::v3::QueryPlanV3;
        use type_bridge_contract::query_plan::{InputColumn, InputColumnId};
        use type_bridge_contract::query_remote_v3::{StagedPlanV3, StagedResultRefV3};
        let mut program = Self::new(miss)?;
        let probe = probes.last().ok_or_else(identity_error)?;
        let QueryOutput::Rows { columns } = probe.local().output() else {
            return Err(identity_error());
        };
        let expected_types = probe
            .local()
            .binding_types(columns[1])
            .ok_or_else(identity_error)?
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut entries = Vec::with_capacity(
            program.entries.len() + guards.len() + probes.len() + final_reads.len(),
        );
        let mut prefix = VecDeque::with_capacity(guards.len() + probes.len());
        for guard in guards {
            let mut entry = StagedPlanV3::new(
                guard.plan.local(),
                guard.plan.local().given_rows().unwrap_or_default().to_vec(),
                program.authority.declared(),
            )?;
            if let Some(source) = entries.last() {
                entry = gate(entry, source, entries.len() - 1, &program.authority)?;
            }
            entries.push(entry);
            prefix.push_back(StaticStep {
                complete: false,
                ordinal: guard.ordinal,
                missing_iid_noop: false,
                expected_iid: guard.expected_iid,
                reference_resolution: guard.reference_resolution,
            });
        }
        for (position, probe) in probes.iter().enumerate() {
            let mut entry = StagedPlanV3::new(
                probe.local(),
                probe.local().given_rows().unwrap_or_default().to_vec(),
                program.authority.declared(),
            )?;
            if let Some(source) = entries.last() {
                entry = if position == 0 {
                    gate(entry, source, entries.len() - 1, &program.authority)?
                } else {
                    root_source(entry, source, entries.len() - 1, &program.authority)?
                };
            }
            entries.push(entry);
            prefix.push_back(StaticStep {
                complete: false,
                ordinal,
                missing_iid_noop: true,
                expected_iid: None,
                reference_resolution: None,
            });
        }
        let probe_index = entries.len().checked_sub(1).ok_or_else(identity_error)?;
        for (index, entry) in program.entries.into_iter().enumerate() {
            let plan = entry.plan();
            let mut inputs = plan.inputs().to_vec();
            let mut pipeline = plan.pipeline().to_vec();
            let mut rows = entry.rows().to_vec();
            let mut refs = entry.result_refs().to_vec();
            if index == 0 {
                let input =
                    InputColumnId::new(u16::try_from(inputs.len()).map_err(|_| identity_error())?);
                inputs.push(InputColumn::new(
                    input,
                    QueryVariable::new("matched_iid")?,
                    ValueTypeTag::String,
                    true,
                ));
                for row in &mut rows {
                    let mut cells = row.values().to_vec();
                    cells.push(None);
                    *row = InputRow::new(cells);
                }
                let Some(Stage::Match { patterns }) = pipeline.first_mut() else {
                    return Err(identity_error());
                };
                patterns.push(
                    QueryPattern::Not {
                        patterns: vec![QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Input { column: input },
                            right: QueryOperand::Input { column: input },
                        }],
                    }
                    .into(),
                );
                refs.push(StagedResultRefV3 {
                    input,
                    source_plan: u32::try_from(probe_index).map_err(|_| identity_error())?,
                    source_column: 1,
                    expected_types: expected_types.clone(),
                });
            } else {
                for reference in &mut refs {
                    reference.source_plan = reference
                        .source_plan
                        .checked_add(u32::try_from(probe_index + 1).map_err(|_| identity_error())?)
                        .ok_or_else(identity_error)?;
                    let column = usize::from(reference.input.get());
                    let input = &inputs[column];
                    inputs[column] = InputColumn::new(
                        input.id(),
                        input.public_name().clone(),
                        input.value_type(),
                        true,
                    );
                }
            }
            let plan = QueryPlanV3::new_with_limits(
                crate::plan_builders::PlanSpec {
                    bindings: plan.bindings().to_vec(),
                    inputs,
                    given_rows: None,
                    operation: plan.operation(),
                    pipeline,
                    output: plan.output().clone(),
                    managed_semantics: plan.managed_semantics().clone(),
                    compatibility: plan.compatibility().cloned(),
                    functions: plan.functions().to_vec(),
                },
                program.authority.declared(),
                plan.limits(),
            )?;
            entries.push(StagedPlanV3::new_with_result_refs(
                &plan,
                rows,
                refs,
                program.authority.declared(),
            )?);
        }
        for step in &mut program.steps {
            step.complete = false;
            step.missing_iid_noop = true;
        }
        prefix.append(&mut program.steps);
        program.steps = prefix;
        for (index, final_read) in final_reads.iter().enumerate() {
            let mut entry = StagedPlanV3::new(
                final_read.local(),
                final_read.local().given_rows().unwrap_or_default().to_vec(),
                program.authority.declared(),
            )?;
            if index != 0 {
                entry = root_source(
                    entry,
                    entries.last().ok_or_else(identity_error)?,
                    entries.len() - 1,
                    &program.authority,
                )?;
            }
            entries.push(entry);
            let complete = index + 1 == final_reads.len();
            program.steps.push_back(StaticStep {
                complete,
                ordinal,
                missing_iid_noop: !complete,
                expected_iid: None,
                reference_resolution: None,
            });
        }
        type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(
            &entries,
            program.authority.declared(),
        )?;
        program.entries = entries;
        Ok(program)
    }

    pub(super) fn guard_contexts(&self) -> BTreeMap<(usize, u16), ReferenceContext> {
        let mut contexts = BTreeMap::new();
        for (index, step) in self.steps.iter().enumerate() {
            if let Some(resolution) = &step.reference_resolution {
                if let Some(guard) = &resolution.guard {
                    for (column, context) in guard.cells() {
                        contexts.insert((index, column), ReferenceContext::Guard(context));
                    }
                } else {
                    for (column, ordinal) in resolution.ordinals.iter().enumerate() {
                        contexts.insert(
                            (
                                index,
                                u16::try_from(column * 2 + 1).expect("validated reference column"),
                            ),
                            ReferenceContext::Snapshot(*ordinal),
                        );
                    }
                }
            }
        }
        contexts
    }

    pub(super) fn guard_fits(
        plan: &BuiltPlan,
        source: &BuiltPlan,
        authority: &PlanAuthority,
    ) -> Result<(), Diagnostic> {
        use type_bridge_contract::query_remote_v3::StagedPlanV3;
        let entry = |plan: &BuiltPlan| {
            StagedPlanV3::new(
                plan.local(),
                plan.local().given_rows().unwrap_or_default().to_vec(),
                authority.declared(),
            )
        };
        gate(entry(plan)?, &entry(source)?, 0, authority).map(|_| ())
    }

    pub(super) fn staged_entries(
        &self,
    ) -> Vec<type_bridge_contract::query_remote_v3::StagedPlanV3> {
        self.entries.clone()
    }

    pub(super) fn statement_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn completion_count(&self) -> usize {
        self.steps.iter().filter(|step| step.complete).count()
    }

    pub(super) fn records_answers(&self) -> bool {
        !self.steps.is_empty()
    }

    pub(super) fn record_answer(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        if self.records_answers() {
            let answer = answer.to_staged_v3(step.plan.local())?;
            if !step.complete && step.reference_resolution.is_none() {
                let StagedAnswerV3::Rows { rows } = &answer else {
                    return Err(identity_error());
                };
                if rows.is_empty() && !step.missing_iid_noop {
                    return Err(identity_error());
                }
                for row in rows {
                    let [
                        RemoteValue::Value {
                            value: CanonicalValue::Long(ordinal),
                        },
                        identity,
                    ] = row.as_slice()
                    else {
                        return Err(identity_error());
                    };
                    if usize::try_from(*ordinal).ok() != Some(step.ordinal) {
                        return Err(identity_error());
                    }
                    match identity {
                        RemoteValue::Thing { iid, .. } => step.check_identity(iid)?,
                        RemoteValue::Absent if step.missing_iid_noop => {}
                        _ => return Err(identity_error()),
                    }
                }
            }
            self.answers.push(answer);
        }
        Ok(())
    }

    pub(super) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        let Some(step) = self.steps.pop_front() else {
            return Ok(None);
        };
        let entry = &self.entries[self.answers.len()];
        let plan = entry.bind_results(&self.answers, self.authority.declared())?;
        let target_ref = entry.result_refs().iter().find(|reference| {
            entry.plan().inputs()[usize::from(reference.input.get())]
                .public_name()
                .as_str()
                == "target_iid"
        });
        let expected_iid = if let Some(reference) = target_ref {
            let column = usize::from(reference.input.get());
            let mut iids = plan.given_rows().into_iter().flatten().filter_map(|row| {
                match row.values()[column].as_ref() {
                    Some(CanonicalValue::String(iid)) => Some(iid.as_str()),
                    _ => None,
                }
            });
            iids.next().and_then(|first| {
                iids.all(|iid| iid.eq_ignore_ascii_case(first))
                    .then(|| first.to_owned())
            })
        } else {
            step.expected_iid
        };
        Ok(Some(WriteStep {
            plan: BuiltPlan {
                plan,
                verified: self.authority.verified(),
            },
            complete: step.complete,
            ordinal: step.ordinal,
            expected_iid,
            missing_iid_noop: step.missing_iid_noop,
            candidate_filter: false,
            reference_guard: step
                .reference_resolution
                .as_ref()
                .is_some_and(|resolution| resolution.guard.is_some()),
            reference_resolution: step.reference_resolution,
        }))
    }
}

fn root_source(
    entry: type_bridge_contract::query_remote_v3::StagedPlanV3,
    source: &type_bridge_contract::query_remote_v3::StagedPlanV3,
    source_index: usize,
    authority: &PlanAuthority,
) -> Result<type_bridge_contract::query_remote_v3::StagedPlanV3, Diagnostic> {
    use type_bridge_contract::query_remote_v3::{StagedPlanV3, StagedResultRefV3};
    let QueryOutput::Rows { columns } = source.plan().output() else {
        return Err(identity_error());
    };
    let input = entry
        .plan()
        .inputs()
        .iter()
        .find(|input| input.public_name().as_str() == "target_iid")
        .ok_or_else(identity_error)?
        .id();
    let mut refs = entry.result_refs().to_vec();
    refs.push(StagedResultRefV3 {
        input,
        source_plan: u32::try_from(source_index).map_err(|_| identity_error())?,
        source_column: 1,
        expected_types: source
            .plan()
            .binding_types(*columns.get(1).ok_or_else(identity_error)?)
            .ok_or_else(identity_error)?
            .iter()
            .cloned()
            .collect(),
    });
    StagedPlanV3::new_with_result_refs(
        entry.plan(),
        entry.rows().to_vec(),
        refs,
        authority.declared(),
    )
}

fn gate(
    entry: type_bridge_contract::query_remote_v3::StagedPlanV3,
    source: &type_bridge_contract::query_remote_v3::StagedPlanV3,
    source_index: usize,
    authority: &PlanAuthority,
) -> Result<type_bridge_contract::query_remote_v3::StagedPlanV3, Diagnostic> {
    use type_bridge_contract::migration_assertion::QueryVariable;
    use type_bridge_contract::query_plan::InputColumn;
    use type_bridge_contract::query_remote_v3::{StagedPlanV3, StagedResultRefV3};
    let QueryOutput::Rows { columns } = source.plan().output() else {
        return Err(identity_error());
    };
    let plan = entry.plan();
    let mut inputs = plan.inputs().to_vec();
    let mut rows = entry
        .rows()
        .iter()
        .map(|row| row.values().to_vec())
        .collect::<Vec<_>>();
    let mut pipeline = plan.pipeline().to_vec();
    let Some(Stage::Match { patterns }) = pipeline.first_mut() else {
        return Err(identity_error());
    };
    let mut refs = entry.result_refs().to_vec();
    for index in (1..columns.len()).step_by(2) {
        let input = type_bridge_contract::query_plan::InputColumnId::new(
            u16::try_from(inputs.len()).map_err(|_| identity_error())?,
        );
        inputs.push(InputColumn::new(
            input,
            QueryVariable::new(format!("guard_iid{}", input.get()))?,
            ValueTypeTag::String,
            false,
        ));
        for row in &mut rows {
            row.push(None);
        }
        patterns.push(present(&QueryOperand::Input { column: input }).into());
        refs.push(StagedResultRefV3 {
            input,
            source_plan: u32::try_from(source_index).map_err(|_| identity_error())?,
            source_column: u16::try_from(index).map_err(|_| identity_error())?,
            expected_types: source
                .plan()
                .binding_types(columns[index])
                .ok_or_else(identity_error)?
                .iter()
                .cloned()
                .collect(),
        });
    }
    if plan.bindings().len() + inputs.len() > authority.query_limits().structural.bindings {
        return Err(invalid(
            "query_plan_binding_limit",
            "Guard dependencies exceed the configured binding ceiling",
        ));
    }
    let plan = type_bridge_contract::query_plan::v3::QueryPlanV3::new_with_limits(
        crate::plan_builders::PlanSpec {
            bindings: plan.bindings().to_vec(),
            inputs,
            given_rows: None,
            operation: plan.operation(),
            pipeline,
            output: plan.output().clone(),
            managed_semantics: plan.managed_semantics().clone(),
            compatibility: plan.compatibility().cloned(),
            functions: plan.functions().to_vec(),
        },
        authority.declared(),
        plan.limits(),
    )?;
    StagedPlanV3::new_with_result_refs(
        &plan,
        rows.into_iter().map(InputRow::new).collect(),
        refs,
        authority.declared(),
    )
}

#[cfg(test)]
mod static_tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;

    #[test]
    fn compact_source_layout_requires_iid_or_common_inherited_uniqueness() {
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents};
        let yaml = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  email: { value: string }
  tag: { value: string }
entities:
  party:
    abstract: true
    owns:
      identifier: { key: true }
      email: { unique: true, card: { min: 0, max: 1 } }
      tag: { card: { min: 0, max: 1 } }
  person:
    sub: party
  employee:
    sub: person
  other:
    owns:
      identifier: { key: true }
relations:
  edge:
    relates:
      member: { card: { min: 0 } }
plays:
  party:
    edge: [member]
  other:
    edge: [member]
"#;
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("source-uniqueness.yaml").unwrap(), yaml)])
                .unwrap(),
        )
        .unwrap();
        let authority = PlanAuthority::new(
            declared,
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"source-uniqueness",
            )
            .unwrap(),
        );
        for (types, attribute, iid, expected) in [
            (&["person", "employee"][..], "identifier", false, true),
            (&["employee"][..], "identifier", false, true),
            (&["person", "other"][..], "identifier", false, false),
            (&["person"][..], "tag", false, false),
            (&["person"][..], "email", false, true),
            (&["person"][..], "tag", true, true),
            (&[][..], "identifier", false, false),
            (&["other"][..], "email", false, false),
        ] {
            let player = Player {
                role: RoleId::new("edge", "member").unwrap(),
                candidates: types
                    .iter()
                    .map(|name| TypeId::new(TypeKind::Entity, *name).unwrap())
                    .collect(),
                iid: iid.then(|| "0x10".into()),
                key: (!iid).then(|| {
                    (
                        AttributeId::new(attribute).unwrap(),
                        CanonicalValue::String(
                            type_bridge_contract::value::CanonicalString::new("shared").unwrap(),
                        ),
                    )
                }),
            };
            assert_eq!(
                super::super::unique_player_identity(&authority, &player),
                expected,
                "types={types:?}, attribute={attribute}, iid={iid}"
            );
        }
    }

    #[test]
    fn mixed_scalar_full_match_keeps_field_value_pairs_and_bounds_boolean_branches() {
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_contract::value::CanonicalString;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents};
        let yaml = "format: typebridge.schema/v2\nattributes:\n  mixed-tag: { value: string }\n  mixed-alias: { value: string }\n  mixed-number: { value: integer }\nentities:\n  note:\n    owns:\n      mixed-tag: { card: { min: 0 } }\n      mixed-alias: { card: { min: 0 } }\n      mixed-number: { card: { min: 0 } }\n";
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("mixed-scalar.yaml").unwrap(), yaml)])
                .unwrap(),
        )
        .unwrap();
        let note = TypeId::new(TypeKind::Entity, "note").unwrap();
        let mut fields = (0..300)
            .map(|index| {
                let (name, value) = match index % 3 {
                    0 => (
                        "mixed-tag",
                        CanonicalValue::String(
                            CanonicalString::new(format!("v-{}", index / 3)).unwrap(),
                        ),
                    ),
                    1 => (
                        "mixed-alias",
                        CanonicalValue::String(
                            CanonicalString::new(format!("v-{}", index / 3)).unwrap(),
                        ),
                    ),
                    _ => ("mixed-number", CanonicalValue::Long(index / 3)),
                };
                (
                    OwnsFactId::new(note.clone(), AttributeId::new(name).unwrap()).unwrap(),
                    value,
                )
            })
            .collect::<Vec<_>>();
        fields.extend([fields[0].clone(), fields[1].clone()]);
        let authority = PlanAuthority::new(
            declared,
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"mixed-scalar-match",
            )
            .unwrap(),
        );
        for terms in [256, 8, 3] {
            let mut limits = type_bridge_contract::limits::QueryLimits::V3;
            limits.structural.boolean_terms = terms;
            let authority = authority
                .clone()
                .with_query_limits(std::sync::Arc::new(limits));
            let WriteProgram::Static(program) = super::super::match_put::wide_match(
                &authority,
                WriteShape {
                    type_id: &note,
                    keys: &[],
                    cleared_fields: &[],
                    cleared_roles: &[],
                    document_roles: vec![],
                    include_subtypes: false,
                    return_documents: true,
                    references_see_prior_writes: true,
                    single_input: false,
                },
                &WriteRow {
                    iid: None,
                    fields: &fields,
                    players: &[],
                },
                0,
            )
            .unwrap() else {
                panic!("mixed fields require a static program")
            };
            let probe = &program.entries[0];
            assert_eq!(probe.rows().len(), 300);
            assert_eq!(probe.plan().bindings().len(), 5);
            assert_eq!(probe.plan().inputs().len(), 1);
            assert_eq!(
                probe
                    .rows()
                    .iter()
                    .map(|row| row.values()[0].clone().unwrap())
                    .collect::<Vec<_>>(),
                (0..300).map(CanonicalValue::Long).collect::<Vec<_>>()
            );
            assert!(probe.plan().pipeline().iter().any(|stage| matches!(stage, Stage::Match { patterns } if patterns.iter().any(|pattern| matches!(pattern, Pattern::Read(QueryPattern::Value { right: QueryOperand::Literal { value: CanonicalValue::Long(300) }, .. }))))));
            let plan = probe.bound_plan(authority.declared()).unwrap();
            let query = crate::query_plan_exec::staged_query(&plan, true).unwrap();
            assert!(!query.writes);
            for name in ["mixed-tag", "mixed-alias", "mixed-number"] {
                assert!(query.typeql.contains(name));
            }
            assert!(!program.entries.last().unwrap().plan().writes());
        }
        for resource in [
            "input_rows",
            "predicate_nodes",
            "canonical_bytes",
            "collections",
        ] {
            let mut limits = type_bridge_contract::limits::QueryLimits::V3;
            match resource {
                "input_rows" => limits.structural.input_rows = 2,
                "predicate_nodes" => limits.structural.predicate_nodes = 128,
                "canonical_bytes" => limits.codec.max_bytes = 20_000,
                _ => limits.codec.max_collection_len = 128,
            }
            let authority = authority
                .clone()
                .with_query_limits(std::sync::Arc::new(limits));
            let WriteProgram::Static(program) = super::super::match_put::wide_match(
                &authority,
                WriteShape {
                    type_id: &note,
                    keys: &[],
                    cleared_fields: &[],
                    cleared_roles: &[],
                    document_roles: vec![],
                    include_subtypes: false,
                    return_documents: true,
                    references_see_prior_writes: true,
                    single_input: false,
                },
                &WriteRow {
                    iid: None,
                    fields: &fields,
                    players: &[],
                },
                0,
            )
            .unwrap_or_else(|error| panic!("{resource}: {error:?}")) else {
                panic!("bounded full-match graph")
            };
            let reads = program
                .entries
                .iter()
                .take_while(|entry| !entry.plan().writes())
                .count();
            assert!(reads > 1);
            assert!(
                program
                    .entries
                    .iter()
                    .all(|entry| entry.rows().len() <= limits.structural.input_rows)
            );
            assert!(
                program.entries[reads - 1]
                    .plan()
                    .pipeline()
                    .iter()
                    .any(|stage| matches!(stage, Stage::Limit { rows: 1 }))
            );
            assert!(program.entries[..reads - 1].iter().all(|entry| {
                !entry
                    .plan()
                    .pipeline()
                    .iter()
                    .any(|stage| matches!(stage, Stage::Limit { .. }))
            }));
            assert!(
                !program
                    .entries
                    .last()
                    .unwrap()
                    .plan()
                    .pipeline()
                    .iter()
                    .any(|stage| matches!(stage, Stage::Limit { .. }))
            );
            assert!(program.entries.iter().all(|entry| {
                entry
                    .result_refs()
                    .iter()
                    .map(|reference| reference.source_plan)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    <= 1
            }));
        }
    }

    #[test]
    fn wide_relation_full_match_includes_the_role_identity_in_its_static_proof() {
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_contract::value::CanonicalString;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents};
        let yaml = "format: typebridge.schema/v2\nattributes:\n  selector: { value: string }\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      selector: { card: { min: 0, max: 1 } }\nrelations:\n  edge:\n    owns:\n      tag: { card: { min: 0 } }\n    relates:\n      member: { card: { min: 1, max: 1 } }\nplays:\n  person:\n    edge: [member]\n";
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("static-relation.yaml").unwrap(), yaml)])
                .unwrap(),
        )
        .unwrap();
        let relation = TypeId::new(TypeKind::Relation, "edge").unwrap();
        let field = OwnsFactId::new(relation.clone(), AttributeId::new("tag").unwrap()).unwrap();
        let fields = (0..300)
            .map(|index| {
                (
                    field.clone(),
                    CanonicalValue::String(CanonicalString::new(format!("tag-{index}")).unwrap()),
                )
            })
            .collect::<Vec<_>>();
        let authority = PlanAuthority::new(
            declared,
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"static-relation-match",
            )
            .unwrap(),
        );
        let candidates = [TypeId::new(TypeKind::Entity, "person").unwrap()];
        let selector = AttributeId::new("selector").unwrap();
        for by_iid in [false, true] {
            for terms in [256, 8] {
                let mut limits = type_bridge_contract::limits::QueryLimits::V3;
                limits.structural.boolean_terms = terms;
                let authority = authority
                    .clone()
                    .with_query_limits(std::sync::Arc::new(limits));
                let players = [(
                    RoleId::new("edge", "member").unwrap(),
                    Reference {
                        candidates: &candidates,
                        identity: if by_iid {
                            ReferenceIdentity::Iid("0x10")
                        } else {
                            ReferenceIdentity::Key {
                                attribute: &selector,
                                value: CanonicalValue::String(
                                    CanonicalString::new("shared").unwrap(),
                                ),
                            }
                        },
                    },
                )];
                let compile = || {
                    super::super::match_put::wide_match(
                        &authority,
                        WriteShape {
                            type_id: &relation,
                            keys: &[],
                            cleared_fields: &[],
                            cleared_roles: &[],
                            document_roles: vec![],
                            include_subtypes: false,
                            return_documents: true,
                            references_see_prior_writes: true,
                            single_input: true,
                        },
                        &WriteRow {
                            iid: None,
                            fields: &fields,
                            players: &players,
                        },
                        0,
                    )
                    .unwrap()
                };
                let WriteProgram::Static(program) = compile() else {
                    panic!("scalar and role predicates must form one static proof")
                };
                let probe = &program.entries[0];
                assert_eq!(probe.rows().len(), 300);
                assert_eq!(probe.plan().inputs().len(), 1);
                assert_eq!(probe.plan().bindings().len(), if by_iid { 6 } else { 7 });
                let plan = probe.bound_plan(authority.declared()).unwrap();
                let query = crate::query_plan_exec::staged_query(&plan, true).unwrap();
                assert!(!query.writes);
                assert!(query.typeql.contains("links (member:"));
                assert!(
                    query
                        .typeql
                        .contains(if by_iid { "0x10" } else { "selector" })
                );
                assert!(query.typeql.contains("300"));
                assert!(!program.entries.last().unwrap().plan().writes());
                let local = program.staged_entries();
                let chain =
                    WriteProgram::Chain(VecDeque::from([WriteProgram::Static(program), compile()]));
                let exported = chain.staged_entries(&authority).unwrap().unwrap();
                assert_eq!(exported.len(), 2 * local.len());
                for (block, entries) in exported.chunks(local.len()).enumerate() {
                    for (actual, expected) in entries.iter().zip(&local) {
                        assert_eq!(
                            *actual,
                            expected
                                .clone()
                                .rebase_result_refs((block * local.len()) as u32)
                                .unwrap()
                        );
                        for reference in actual.result_refs() {
                            assert!(actual.rows().iter().all(|row| {
                                row.values()[usize::from(reference.input.get())].is_none()
                            }));
                        }
                    }
                }
                type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(
                    &exported,
                    authority.declared(),
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn wide_scalar_full_match_has_static_optional_hit_and_miss_dependencies() {
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let field = OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let fields = (0..300)
            .map(|index| (field.clone(), CanonicalValue::Long(index)))
            .collect::<Vec<_>>();
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"static-scalar-match",
            )
            .unwrap(),
        );
        for duplicate in [false, true] {
            let mut fields = fields.clone();
            if duplicate {
                fields[299] = fields[0].clone();
            }
            let expected_values = 300 - usize::from(duplicate);
            let WriteProgram::Static(program) = super::super::match_put::wide_match(
                &authority,
                WriteShape {
                    type_id: &account,
                    keys: &[],
                    cleared_fields: &[],
                    cleared_roles: &[],
                    document_roles: vec![],
                    include_subtypes: false,
                    return_documents: false,
                    references_see_prior_writes: true,
                    single_input: false,
                },
                &WriteRow {
                    iid: None,
                    fields: &fields,
                    players: &[],
                },
                0,
            )
            .unwrap() else {
                panic!("static wide scalar match")
            };
            assert_eq!(program.entries[0].rows().len(), expected_values);
            assert!(
                matches!(&program.entries[0].plan().pipeline()[0], Stage::Match { patterns }
            if patterns.iter().any(|pattern| matches!(pattern, Pattern::Read(QueryPattern::Isa { include_subtypes: true, .. }))))
            );
            assert!(program.entries[0].plan().pipeline().iter().any(|stage| matches!(stage, Stage::Match { patterns }
            if patterns.iter().any(|pattern| matches!(pattern, Pattern::Read(QueryPattern::Value { right: QueryOperand::Literal { value: CanonicalValue::Long(count) }, .. }) if *count == expected_values as i64)))));
            let QueryOutput::Rows { columns } = program.entries[3].plan().output() else {
                panic!("common identity row")
            };
            assert_eq!(
                columns
                    .iter()
                    .map(
                        |column| program.entries[3].plan().bindings()[usize::from(column.get())]
                            .variable()
                            .as_str()
                    )
                    .collect::<Vec<_>>(),
                ["ordinal1", "root0"]
            );
            assert_eq!(program.entries.len(), 4);
            assert_eq!(program.completion_count(), 1);
            assert!(
                program.entries[0]
                    .plan()
                    .pipeline()
                    .iter()
                    .any(|stage| matches!(stage, Stage::Reduce { .. }))
            );
            assert!(!program.entries[0].plan().writes());
            assert!(!program.entries[3].plan().writes());
            assert_eq!(program.entries[1].result_refs()[0].source_plan, 0);
            assert_eq!(program.entries[2].result_refs()[0].source_plan, 1);
            for index in [1, 2] {
                let entry = &program.entries[index];
                let input = usize::from(entry.result_refs()[0].input.get());
                assert!(entry.plan().inputs()[input].optional());
                assert!(entry.rows().iter().all(|row| row.values()[input].is_none()));
            }
            let root = |iid: &str| {
                vec![
                    RemoteValue::Value {
                        value: CanonicalValue::Long(0),
                    },
                    RemoteValue::Thing {
                        iid: iid.into(),
                        type_id: account.clone(),
                    },
                ]
            };
            for source in [
                StagedAnswerV3::Rows { rows: vec![] },
                StagedAnswerV3::Rows {
                    rows: vec![root("0x10")],
                },
            ] {
                let plan = program.entries[1]
                    .bind_source_result(&source, authority.declared())
                    .unwrap();
                let input = usize::from(program.entries[1].result_refs()[0].input.get());
                let present = matches!(&source, StagedAnswerV3::Rows { rows } if !rows.is_empty());
                assert_eq!(
                    plan.given_rows().unwrap()[0].values()[input].is_some(),
                    present
                );
            }
            let inherited = program.entries[1]
                .bind_source_result(
                    &StagedAnswerV3::Rows {
                        rows: vec![root("0x10"), root("0x20")],
                    },
                    authority.declared(),
                )
                .unwrap();
            assert_eq!(inherited.given_rows().unwrap().len(), 2);
            let inherited_column = usize::from(program.entries[1].result_refs()[0].input.get());
            assert_eq!(
                inherited.given_rows().unwrap()[0].values()[inherited_column],
                Some(super::super::iid("0x10").unwrap())
            );
            assert_eq!(
                inherited.given_rows().unwrap()[1].values()[inherited_column],
                Some(super::super::iid("0x20").unwrap())
            );
            let absent = program.entries[2]
                .bind_source_result(&StagedAnswerV3::Rows { rows: vec![] }, authority.declared())
                .unwrap();
            let input = usize::from(program.entries[2].result_refs()[0].input.get());
            assert!(
                absent
                    .given_rows()
                    .unwrap()
                    .iter()
                    .all(|row| row.values()[input].is_none())
            );
        }
    }

    #[test]
    fn forced_scalar_pieces_are_authored_references_before_execution() {
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let balance =
            OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let key =
            OwnsFactId::new(account.clone(), AttributeId::new("account-id").unwrap()).unwrap();
        let fields = std::iter::once((
            key.clone(),
            CanonicalValue::String(CanonicalString::new("static-stack").unwrap()),
        ))
        .chain((0..300).map(|value| (balance.clone(), CanonicalValue::Long(value))))
        .collect::<Vec<_>>();
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"static-pieces",
            )
            .unwrap(),
        );
        let mut program = write_many(
            &authority,
            WriteKind::Insert,
            WriteShape {
                references_see_prior_writes: true,
                single_input: false,
                type_id: &account,
                keys: &[key],
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
                include_subtypes: true,
                return_documents: false,
            },
            &[WriteRow {
                iid: None,
                fields: &fields,
                players: &[],
            }],
        )
        .unwrap()
        .prepare_static()
        .unwrap();
        let WriteProgram::Static(stack) = &program else {
            panic!("forced static stack")
        };
        assert!(stack.entries.len() > 1);
        assert!(stack.entries[0].result_refs().is_empty());
        for entry in &stack.entries[1..] {
            let [reference] = entry.result_refs() else {
                panic!("one source")
            };
            assert_eq!((reference.source_plan, reference.source_column), (0, 1));
            assert_eq!(reference.expected_types, std::slice::from_ref(&account));
            assert!(
                entry
                    .rows()
                    .iter()
                    .all(|row| row.values()[usize::from(reference.input.get())].is_none())
            );
        }
        let count = stack.entries.len();
        let wire = StagedAnswerV3::Rows {
            rows: vec![vec![
                RemoteValue::Value {
                    value: CanonicalValue::Long(0),
                },
                RemoteValue::Thing {
                    iid: "0x10".into(),
                    type_id: account,
                },
            ]],
        };
        let mut observed = 0;
        while let Some(step) = program.next().unwrap() {
            if observed != 0 {
                let column = step
                    .plan
                    .local()
                    .inputs()
                    .iter()
                    .position(|input| input.public_name().as_str() == "target_iid")
                    .unwrap();
                assert!(
                    step.plan
                        .local()
                        .given_rows()
                        .unwrap()
                        .iter()
                        .all(|row| row.values()[column]
                            == Some(CanonicalValue::String(
                                CanonicalString::new("0x10").unwrap()
                            )))
                );
            }
            let answer = PlanAnswer::from_staged_v3(&wire, step.plan.local()).unwrap();
            program.record_answer(&step, &answer).unwrap();
            if !step.complete {
                program
                    .accept_identity(&step, step.identity(&answer).unwrap())
                    .unwrap();
            }
            observed += 1;
        }
        assert_eq!(observed, count);
    }
    #[test]
    fn missing_partial_update_keeps_all_static_attachments_empty() {
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let balance =
            OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let fields = (0..300)
            .map(|value| (balance.clone(), CanonicalValue::Long(value)))
            .collect::<Vec<_>>();
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"static-pieces",
            )
            .unwrap(),
        );
        let mut program = write_many(
            &authority,
            WriteKind::PartialUpdate,
            WriteShape {
                references_see_prior_writes: true,
                single_input: false,
                type_id: &account,
                keys: &[],
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
                include_subtypes: true,
                return_documents: false,
            },
            &[WriteRow {
                iid: Some("0x10"),
                fields: &fields,
                players: &[],
            }],
        )
        .unwrap()
        .prepare_static()
        .unwrap();
        assert!(matches!(program, WriteProgram::Static(_)));
        let mut observed = 0;
        while let Some(step) = program.next().unwrap() {
            let wire = if observed == 0 {
                StagedAnswerV3::Rows {
                    rows: vec![vec![
                        RemoteValue::Value {
                            value: CanonicalValue::Long(0),
                        },
                        RemoteValue::Absent,
                    ]],
                }
            } else {
                let column = step
                    .plan
                    .local()
                    .inputs()
                    .iter()
                    .position(|input| input.public_name().as_str() == "target_iid")
                    .unwrap();
                assert!(step.plan.local().inputs()[column].optional());
                assert!(
                    step.plan
                        .local()
                        .given_rows()
                        .unwrap()
                        .iter()
                        .all(|row| row.values()[column].is_none())
                );
                StagedAnswerV3::Rows { rows: vec![] }
            };
            let answer = PlanAnswer::from_staged_v3(&wire, step.plan.local()).unwrap();
            program.record_answer(&step, &answer).unwrap();
            if !step.complete {
                program
                    .accept_identity(&step, step.identity(&answer).unwrap())
                    .unwrap();
            }
            observed += 1;
        }
        assert!(observed > 1);
    }
    #[test]
    fn sequential_static_groups_bind_their_own_source_answers() {
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let balance =
            OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let key =
            OwnsFactId::new(account.clone(), AttributeId::new("account-id").unwrap()).unwrap();
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"static-pieces",
            )
            .unwrap(),
        );
        let build = |name: &str| {
            let fields = std::iter::once((
                key.clone(),
                CanonicalValue::String(CanonicalString::new(name).unwrap()),
            ))
            .chain((0..300).map(|value| (balance.clone(), CanonicalValue::Long(value))))
            .collect::<Vec<_>>();
            write_many(
                &authority,
                WriteKind::Insert,
                WriteShape {
                    references_see_prior_writes: true,
                    single_input: false,
                    type_id: &account,
                    keys: std::slice::from_ref(&key),
                    cleared_fields: &[],
                    cleared_roles: &[],
                    document_roles: vec![],
                    include_subtypes: true,
                    return_documents: false,
                },
                &[WriteRow {
                    iid: None,
                    fields: &fields,
                    players: &[],
                }],
            )
            .unwrap()
        };
        let first = build("first");
        let count = first.statement_count();
        let mut program = WriteProgram::Chain(VecDeque::from([first, build("second")]))
            .prepare_static()
            .unwrap();
        let WriteProgram::Chain(groups) = &program else {
            panic!("sequential groups")
        };
        assert!(
            groups
                .iter()
                .all(|group| matches!(group, WriteProgram::Static(_)))
        );
        let mut observed = 0;
        while let Some(step) = program.next().unwrap() {
            let iid = if observed < count { "0x10" } else { "0x20" };
            if observed % count != 0 {
                let column = step
                    .plan
                    .local()
                    .inputs()
                    .iter()
                    .position(|input| input.public_name().as_str() == "target_iid")
                    .unwrap();
                let expected = Some(CanonicalValue::String(CanonicalString::new(iid).unwrap()));
                assert!(
                    step.plan
                        .local()
                        .given_rows()
                        .unwrap()
                        .iter()
                        .all(|row| row.values()[column] == expected)
                );
            }
            let wire = StagedAnswerV3::Rows {
                rows: vec![vec![
                    RemoteValue::Value {
                        value: CanonicalValue::Long(0),
                    },
                    RemoteValue::Thing {
                        iid: iid.into(),
                        type_id: account.clone(),
                    },
                ]],
            };
            let answer = PlanAnswer::from_staged_v3(&wire, step.plan.local()).unwrap();
            program.record_answer(&step, &answer).unwrap();
            if !step.complete {
                program
                    .accept_identity(&step, step.identity(&answer).unwrap())
                    .unwrap();
            }
            observed += 1;
        }
        assert_eq!(observed, count * 2);
    }
    #[test]
    fn full_match_graph_replays_guarded_and_compact_misses_without_truncating_roots() {
        use type_bridge_contract::query_remote::RemoteValue;
        use type_bridge_contract::query_remote_v3::StagedAnswerV3;
        use type_bridge_contract::value::CanonicalString;
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"full-match-graph-replay",
            )
            .unwrap(),
        );
        let text = |value: &str| CanonicalValue::String(CanonicalString::new(value).unwrap());
        let relation = TypeId::new(TypeKind::Relation, "purchase").unwrap();
        let candidates = [TypeId::new(TypeKind::Entity, "account").unwrap()];
        let role = RoleId::new("purchase", "buyer").unwrap();
        let field =
            OwnsFactId::new(relation.clone(), AttributeId::new("order-id").unwrap()).unwrap();
        let fields = (0..300)
            .map(|index| (field.clone(), text(&format!("v-{index}"))))
            .collect::<Vec<_>>();
        let iids = ["0x02", "0x03", "0x04"];
        let players = iids
            .iter()
            .map(|iid| {
                (
                    role.clone(),
                    Reference {
                        candidates: &candidates,
                        identity: ReferenceIdentity::Iid(iid),
                    },
                )
            })
            .collect::<Vec<_>>();
        let key = AttributeId::new("account-id").unwrap();
        let many = (0..100)
            .map(|index| {
                (
                    role.clone(),
                    Reference {
                        candidates: &candidates,
                        identity: ReferenceIdentity::Key {
                            attribute: &key,
                            value: text(&format!("p-{index}")),
                        },
                    },
                )
            })
            .collect::<Vec<_>>();
        for input_rows in [
            type_bridge_contract::limits::QueryLimits::V3
                .structural
                .input_rows,
            2,
        ] {
            let mut limits = type_bridge_contract::limits::QueryLimits::V3;
            limits.structural.input_rows = input_rows;
            let authority = authority
                .clone()
                .with_query_limits(std::sync::Arc::new(limits));
            for (fields, players, guarded) in [
                (fields.as_slice(), players.as_slice(), true),
                (&[][..], many.as_slice(), false),
            ] {
                for hit in [false, true] {
                    let mut program = super::super::match_put::wide_match(
                        &authority,
                        WriteShape {
                            references_see_prior_writes: true,
                            single_input: false,
                            type_id: &relation,
                            keys: &[],
                            cleared_fields: &[],
                            cleared_roles: &[],
                            document_roles: vec![],
                            include_subtypes: true,
                            return_documents: false,
                        },
                        &WriteRow {
                            iid: None,
                            fields,
                            players,
                        },
                        0,
                    )
                    .unwrap();
                    let entries = program
                        .staged_entries(&authority)
                        .unwrap()
                        .expect("a complete fixed graph");
                    type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(
                        &entries,
                        authority.declared(),
                    )
                    .unwrap();
                    if input_rows != 2 {
                        let probe_index = usize::from(guarded);
                        if guarded {
                            assert_eq!(entries[1].result_refs().len(), 2);
                        }
                        assert_eq!(
                            entries[probe_index + 1].result_refs()[0].source_plan,
                            probe_index as u32
                        );
                        if !guarded {
                            assert_eq!(
                                entries[1].rows().len(),
                                1,
                                "the hit gate preserves the compact allocation's single input row"
                            );
                        }
                    }
                    let expected_roots = if hit {
                        vec!["0x20", "0x22"]
                    } else {
                        vec!["0x10"]
                    };
                    let mut completed = Vec::new();
                    let mut probes = 0;
                    let mut after_write = false;
                    while let Some(step) = program.next().unwrap() {
                        let identity_row = |iid: &str| {
                            vec![
                                RemoteValue::Value {
                                    value: CanonicalValue::Long(0),
                                },
                                RemoteValue::Thing {
                                    iid: iid.into(),
                                    type_id: relation.clone(),
                                },
                            ]
                        };
                        let rows = if let Some(resolution) = &step.reference_resolution {
                            vec![
                                resolution
                                    .ordinals
                                    .iter()
                                    .flat_map(|ordinal| {
                                        [
                                            RemoteValue::Value {
                                                value: CanonicalValue::Long(*ordinal as i64),
                                            },
                                            RemoteValue::Thing {
                                                iid: iids[*ordinal].into(),
                                                type_id: candidates[0].clone(),
                                            },
                                        ]
                                    })
                                    .collect(),
                            ]
                        } else if step.plan.local().writes() {
                            after_write = true;
                            if hit {
                                vec![]
                            } else {
                                vec![identity_row("0x10")]
                            }
                        } else if step.complete {
                            expected_roots.iter().map(|iid| identity_row(iid)).collect()
                        } else {
                            probes += 1;
                            let witness = step.plan.local().pipeline().iter().any(|stage| {
                                matches!(
                                    stage,
                                    type_bridge_contract::query_plan::v3::Stage::Limit { rows: 1 }
                                )
                            });
                            if witness {
                                if hit {
                                    vec![identity_row("0x20")]
                                } else {
                                    vec![]
                                }
                            } else {
                                assert_eq!(input_rows, 2);
                                if after_write || hit {
                                    expected_roots.iter().map(|iid| identity_row(iid)).collect()
                                } else {
                                    vec![]
                                }
                            }
                        };
                        let answer = crate::query_plan_exec::PlanAnswer::from_staged_v3(
                            &StagedAnswerV3::Rows { rows },
                            step.plan.local(),
                        )
                        .unwrap();
                        if step.complete {
                            assert!(!step.plan.local().pipeline().iter().any(|stage| matches!(
                                stage,
                                type_bridge_contract::query_plan::v3::Stage::Limit { .. }
                            )));
                            completed = step
                                .reference_iids(&answer)
                                .unwrap()
                                .into_iter()
                                .map(|(_, iid)| iid)
                                .collect();
                        }
                        if let Some(resolution) = &step.reference_resolution {
                            resolution
                                .guard
                                .as_ref()
                                .unwrap()
                                .validate(&step.reference_iids(&answer).unwrap(), |_, _| None)
                                .unwrap();
                        }
                        program.record_answer(&step, &answer).unwrap();
                    }
                    if input_rows == 2 {
                        assert!(probes > 1);
                    } else {
                        assert_eq!(probes, 1);
                    }
                    assert_eq!(completed, expected_roots);
                }
            }
        }
    }
}
