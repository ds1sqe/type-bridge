//! Freeze mutable reference identities before an explicit batch's forced pieces.
use super::*;

pub(crate) struct ResolutionStep {
    pub(crate) ordinals: Vec<usize>,
    pub(crate) guard: Option<GuardContext>,
}

pub(crate) struct GuardContext {
    relation: TypeId,
    row: usize,
    references: Vec<(usize, RoleId, usize, TypeId, Option<String>)>,
}
impl GuardContext {
    pub(crate) fn row(&self) -> usize {
        self.row
    }
    pub(super) fn cells(&self) -> impl Iterator<Item = (u16, Self)> + '_ {
        self.references
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                (
                    u16::try_from(index * 2 + 1).expect("validated output column"),
                    Self {
                        relation: self.relation.clone(),
                        row: self.row,
                        references: vec![reference.clone()],
                    },
                )
            })
    }

    pub(crate) fn validate(
        &self,
        answers: &[(usize, String)],
        requested_type: impl FnOnce(&RoleId, usize) -> Option<TypeId>,
    ) -> Result<(), type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
        use crate::projected_batch_executor::{integrity_at_reference, integrity_at_row};
        let malformed = || {
            integrity_at_row(
                "provider_identity_mismatch",
                "Reference guard returned repeated or foreign identity evidence",
                &self.relation,
                self.row,
            )
        };
        let mut seen = std::collections::BTreeSet::new();
        for (ordinal, iid) in answers {
            let index = self
                .references
                .binary_search_by_key(ordinal, |reference| reference.0)
                .map_err(|_| malformed())?;
            let reference = &self.references[index];
            if !seen.insert(*ordinal)
                || reference
                    .4
                    .as_ref()
                    .is_some_and(|expected| !expected.eq_ignore_ascii_case(iid))
            {
                return Err(malformed());
            }
        }
        if let Some((_, role, reference, player, _)) = self
            .references
            .iter()
            .find(|reference| !seen.contains(&reference.0))
        {
            return Err(integrity_at_reference(
                "projected_batch_reference_missing",
                "A projected role-player reference did not resolve exactly once",
                &self.relation,
                role,
                self.row,
                *reference,
                &requested_type(role, *reference).unwrap_or_else(|| player.clone()),
            ));
        }
        Ok(())
    }
}

pub(super) fn guards(
    authority: &PlanAuthority,
    relation: &TypeId,
    players: &[Player],
    row: usize,
    seed: &BuiltPlan,
) -> Result<Vec<WriteStep>, Diagnostic> {
    let mut routes: Vec<(LookupRoute, Vec<InputRow>)> = Vec::new();
    let mut references = Vec::new();
    let mut indices = BTreeMap::<RoleId, usize>::new();
    for (ordinal, player) in players.iter().enumerate() {
        let reference = indices.entry(player.role.clone()).or_default();
        let index = *reference;
        *reference += 1;
        // The seed proves its first player atomically; the rest must exist before writes.
        if ordinal == 0 {
            continue;
        }
        let value = if let Some(iid) = &player.iid {
            super::iid(iid)?
        } else {
            player.key.as_ref().ok_or_else(identity_error)?.1.clone()
        };
        let route = LookupRoute {
            candidates: player.candidates.clone(),
            key: player.key.as_ref().map(|(key, _)| key.clone()),
        };
        // Preserve first-missing-reference precedence across different routes.
        if routes.last().is_none_or(|(previous, _)| previous != &route) {
            routes.push((route, Vec::new()));
        }
        routes
            .last_mut()
            .expect("route added")
            .1
            .push(InputRow::new(vec![
                Some(CanonicalValue::Long(
                    i64::try_from(ordinal).map_err(|_| identity_error())?,
                )),
                Some(value),
            ]));
        references.push((
            ordinal,
            player.role.clone(),
            index,
            player
                .candidates
                .first()
                .ok_or_else(identity_error)?
                .clone(),
            player.iid.clone(),
        ));
    }
    let mut guards: Vec<WriteStep> = Vec::new();
    for (route, rows) in routes {
        let mut offset = 0;
        let mut validated_seed_width = 0;
        while offset < rows.len() {
            let mut low = 0;
            let mut high = rows.len() - offset;
            while low < high {
                let count = low + (high - low).div_ceil(2);
                let proof = guard_proof(authority, &route, &rows[offset..offset + count]).and_then(
                    |proof| {
                        if let Some(previous) = guards.last() {
                            StaticProgram::guard_fits(&proof, &previous.plan, authority)?;
                        }
                        // This route has identical output domains at every width.
                        if count > validated_seed_width {
                            StaticProgram::guard_fits(seed, &proof, authority)?;
                            validated_seed_width = count;
                        }
                        Ok(proof)
                    },
                );
                match proof {
                    Ok(_) => low = count,
                    Err(error) if forces_pieces(&error) => high = count - 1,
                    Err(error) => return Err(error),
                }
            }
            if low == 0 {
                let proof = guard_proof(authority, &route, &rows[offset..offset + 1])?;
                if let Some(previous) = guards.last() {
                    StaticProgram::guard_fits(&proof, &previous.plan, authority)?;
                }
                StaticProgram::guard_fits(seed, &proof, authority)?;
                return Err(identity_error());
            }
            let chunk = &rows[offset..offset + low];
            let ordinals = chunk
                .iter()
                .map(|row| match row.values()[0].as_ref() {
                    Some(CanonicalValue::Long(ordinal)) => {
                        usize::try_from(*ordinal).map_err(|_| identity_error())
                    }
                    _ => Err(identity_error()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let selected = references
                .iter()
                .filter(|reference| ordinals.binary_search(&reference.0).is_ok())
                .cloned()
                .collect();
            guards.push(WriteStep {
                plan: guard_proof(authority, &route, chunk)?,
                complete: false,
                ordinal: row,
                expected_iid: None,
                missing_iid_noop: false,
                candidate_filter: false,
                reference_guard: true,
                reference_resolution: Some(Box::new(ResolutionStep {
                    ordinals,
                    guard: Some(GuardContext {
                        relation: relation.clone(),
                        row,
                        references: selected,
                    }),
                })),
            });
            offset += low;
        }
    }
    Ok(guards)
}

// Any witnesses prove existence; the attachment still matches every player.

fn guard_proof(
    authority: &PlanAuthority,
    route: &LookupRoute,
    rows: &[InputRow],
) -> Result<BuiltPlan, Diagnostic> {
    reference_tuple(authority, rows.iter().map(|row| (route, row)))
}

fn reference_tuple<'a>(
    authority: &PlanAuthority,
    cells: impl IntoIterator<Item = (&'a LookupRoute, &'a InputRow)>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(ValueTransport::Given);
    let mut patterns = Vec::new();
    let mut columns = Vec::new();
    for (route, row) in cells {
        let ordinal = draft.bind("reference_ordinal")?;
        let root = draft.bind("reference")?;
        let value = draft.value(row.values()[1].clone().ok_or_else(identity_error)?)?;
        patterns.push(Pattern::Let {
            binding: ordinal,
            expr: Expr::Operand(QueryOperand::Literal {
                value: row.values()[0].clone().ok_or_else(identity_error)?,
            }),
        });
        let branches = route
            .candidates
            .iter()
            .map(|candidate| {
                reference_patterns(&mut draft, root, candidate, route.key.as_ref(), &value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = if branches.len() == 1 {
            branches.into_iter().next().expect("one branch")
        } else {
            vec![QueryPattern::Or { branches }]
        };
        patterns.push(QueryPattern::Try { patterns: body }.into());
        columns.extend([ordinal, root]);
    }
    if draft.bindings.len() + draft.inputs.len() > authority.query_limits().structural.bindings {
        return Err(invalid(
            "query_plan_binding_limit",
            "Reference proof variables exceed the configured binding ceiling",
        ));
    }
    draft.finish(
        authority,
        vec![
            Stage::Match { patterns },
            Stage::Select {
                bindings: columns.clone(),
            },
            // Any witness suffices; the constant ordinal supplies explicit ordering.
            Stage::Sort {
                terms: vec![type_bridge_contract::query_plan::OrderTerm::new(
                    columns[0],
                    type_bridge_contract::query_plan::OrderDirection::Ascending,
                )],
            },
            Stage::Limit { rows: 1 },
        ],
        Operation::Rows,
        QueryOutput::Rows { columns },
    )
}

#[derive(Clone)]
struct OwnedShape {
    type_id: TypeId,
    keys: Vec<OwnsFactId>,
    cleared_fields: Vec<OwnsFactId>,
    cleared_roles: Vec<RoleId>,
    document_roles: Vec<DocumentRole>,
    include_subtypes: bool,
    return_documents: bool,
    single_input: bool,
}
impl OwnedShape {
    fn of(shape: &WriteShape<'_>) -> Self {
        Self {
            type_id: shape.type_id.clone(),
            keys: shape.keys.to_vec(),
            cleared_fields: shape.cleared_fields.to_vec(),
            cleared_roles: shape.cleared_roles.to_vec(),
            document_roles: shape.document_roles.clone(),
            include_subtypes: shape.include_subtypes,
            return_documents: shape.return_documents,
            single_input: shape.single_input,
        }
    }
    fn borrowed(&self) -> WriteShape<'_> {
        WriteShape {
            type_id: &self.type_id,
            keys: &self.keys,
            cleared_fields: &self.cleared_fields,
            cleared_roles: &self.cleared_roles,
            document_roles: self.document_roles.clone(),
            include_subtypes: self.include_subtypes,
            return_documents: self.return_documents,
            references_see_prior_writes: false,
            single_input: self.single_input,
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct LookupRoute {
    candidates: Vec<TypeId>,
    key: Option<AttributeId>,
}

pub(crate) struct ReferenceProgram {
    authority: PlanAuthority,
    shape: OwnedShape,
    kind: WriteKind,
    rows: Vec<Row>,
    locations: Vec<(usize, usize)>,
    resolved: Vec<Option<String>>,
    lookups: VecDeque<(BuiltPlan, Vec<usize>)>,
    body: Option<WriteProgram>,
    snapshots: Option<Box<ReferenceSnapshots>>,
    statements: usize,
    body_ceiling: usize,
    frozen: bool,
}
impl ReferenceProgram {
    pub(super) fn prepare_static(&mut self) -> Result<(), Diagnostic> {
        let supported = match self.kind {
            WriteKind::Update => self.rows.iter().all(|row| row.iid.is_some()),
            WriteKind::Insert => true,
            WriteKind::Put => {
                self.rows.iter().all(|row| row.iid.is_none())
                    && self.rows.iter().flat_map(|row| &row.players).all(|player| {
                        // A keyed put preserves its schema keys; other unique values may change.
                        !player.candidates.contains(&self.shape.type_id)
                            || player.key.as_ref().is_none_or(|(attribute, _)| {
                                self.shape
                                    .keys
                                    .iter()
                                    .any(|key| key.attribute() == attribute)
                            })
                    })
            }
            _ => false,
        };
        if !supported
            || self
                .rows
                .iter()
                .flat_map(|row| &row.players)
                .any(|player| !unique_player_identity(&self.authority, player))
        {
            return Ok(());
        }
        let prepared = self.frozen_program(false).or_else(|error| {
            if forces_pieces(&error) && self.locations.len() > 1 {
                self.frozen_program(true)
            } else {
                Err(error)
            }
        });
        let program = match prepared {
            Ok(program) => program,
            Err(error) if forces_pieces(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        self.statements = program.statement_count();
        self.body = Some(WriteProgram::Static(Box::new(program)));
        self.lookups.clear();
        self.frozen = true;
        Ok(())
    }

    fn frozen_program(&self, framed: bool) -> Result<StaticProgram, Diagnostic> {
        let mut routes = Vec::with_capacity(self.locations.len());
        let mut cells = Vec::with_capacity(self.locations.len());
        let mut placeholders = BTreeMap::new();
        let mut owned = self.rows.clone();
        for (ordinal, &(row, player)) in self.locations.iter().enumerate() {
            let original = &self.rows[row].players[player];
            routes.push(LookupRoute {
                candidates: original.candidates.clone(),
                key: original.key.as_ref().map(|(key, _)| key.clone()),
            });
            let identity = if let Some(iid) = &original.iid {
                super::iid(iid)?
            } else {
                original.key.as_ref().ok_or_else(identity_error)?.1.clone()
            };
            cells.push(InputRow::new(vec![
                Some(CanonicalValue::Long(
                    i64::try_from(ordinal).map_err(|_| identity_error())?,
                )),
                Some(identity),
            ]));
            if self.kind == WriteKind::Update {
                let digits = format!("{ordinal:x}");
                let placeholder = format!(
                    "0x{}{}",
                    "0".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS - digits.len()),
                    digits
                );
                placeholders.insert(placeholder.clone(), ordinal);
                owned[row].players[player].iid = Some(placeholder);
                owned[row].players[player].key = None;
            }
        }
        let lookups = if framed {
            // Keep the first seed's reference last: its required cell gates the
            // first mutation after every earlier snapshot has been proved.
            let final_lookup = reference_tuple(&self.authority, [(&routes[0], &cells[0])])?;
            let mut lookups: Vec<(BuiltPlan, Vec<usize>)> = Vec::new();
            let mut offset = 1;
            while offset < cells.len() {
                let build = |count| {
                    let proof = reference_tuple(
                        &self.authority,
                        routes[offset..offset + count]
                            .iter()
                            .zip(&cells[offset..offset + count]),
                    )?;
                    if let Some((previous, _)) = lookups.last() {
                        StaticProgram::guard_fits(&proof, previous, &self.authority)?;
                    }
                    StaticProgram::guard_fits(&final_lookup, &proof, &self.authority)?;
                    Ok(proof)
                };
                let mut low = 0;
                let mut high = cells.len() - offset;
                while low < high {
                    let count = low + (high - low).div_ceil(2);
                    match build(count) {
                        Ok(_) => low = count,
                        Err(error) if forces_pieces(&error) => high = count - 1,
                        Err(error) => return Err(error),
                    }
                }
                if low == 0 {
                    build(1)?;
                    return Err(identity_error());
                }
                let proof = build(low)?;
                lookups.push((proof, (offset..offset + low).collect()));
                offset += low;
            }
            lookups.push((final_lookup, vec![0]));
            lookups
        } else {
            vec![(
                reference_tuple(&self.authority, routes.iter().zip(&cells))?,
                (0..cells.len()).collect(),
            )]
        };
        let ordinal_frames = lookups
            .iter()
            .enumerate()
            .flat_map(|(index, (_, ordinals))| {
                ordinals.iter().map(move |ordinal| (*ordinal, index))
            })
            .collect::<BTreeMap<_, _>>();
        let frames = placeholders
            .iter()
            .map(|(iid, ordinal)| {
                Ok((
                    iid.clone(),
                    *ordinal_frames.get(ordinal).ok_or_else(identity_error)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
        let mut steps = VecDeque::new();
        for (ordinal, row) in owned.iter().enumerate() {
            let players = player_references(&row.players)?;
            let WriteProgram::Split(program) = split_with_guards(
                &self.authority,
                self.kind,
                self.shape.borrowed(),
                &[WriteRow {
                    iid: row.iid.as_deref(),
                    fields: &row.fields,
                    players: &players,
                }],
                ordinal,
                false,
                (self.kind == WriteKind::Update).then_some(&frames),
            )?
            else {
                return Err(identity_error());
            };
            steps.extend(program.steps);
        }
        StaticProgram::with_reference_lookup(
            SplitProgram {
                steps,
                authority: self.authority.clone(),
                identity: None,
            },
            lookups,
            placeholders,
        )
    }

    pub(super) fn guard_contexts(&self) -> BTreeMap<(usize, u16), ReferenceContext> {
        if self.frozen {
            self.body.as_ref().expect("prepared body").guard_contexts()
        } else {
            BTreeMap::new()
        }
    }
    pub(super) fn staged_entries(
        &self,
        authority: &PlanAuthority,
    ) -> Result<Option<Vec<type_bridge_contract::query_remote_v3::StagedPlanV3>>, Diagnostic> {
        if self.frozen {
            self.body
                .as_ref()
                .expect("prepared body")
                .staged_entries(authority)
        } else {
            Ok(None)
        }
    }

    pub(super) fn records_answers(&self) -> bool {
        self.body
            .as_ref()
            .is_some_and(WriteProgram::records_answers)
    }
    pub(super) fn record_answer(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        if let Some(body) = &mut self.body {
            body.record_answer(step, answer)?;
        }
        Ok(())
    }
    pub(super) fn statement_count(&self) -> usize {
        self.statements
    }
    pub(super) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        if self.frozen {
            if !self.locations.is_empty() && self.resolved.iter().all(Option::is_some) {
                let capture = self.kind == WriteKind::Update
                    && self.rows.iter().any(|row| {
                        row.players.iter().any(|player| {
                            player.key.is_some() && player.candidates.contains(&self.shape.type_id)
                        })
                    });
                let mut snapshots = ReferenceSnapshots::new();
                if capture {
                    for (index, &(row, player)) in self.locations.iter().enumerate() {
                        snapshots
                            .entry(row)
                            .or_default()
                            .entry(self.rows[row].players[player].role.clone())
                            .or_default()
                            .push(
                                self.resolved[index]
                                    .as_ref()
                                    .expect("all resolved")
                                    .to_ascii_lowercase(),
                            );
                    }
                    for roles in snapshots.values_mut() {
                        for iids in roles.values_mut() {
                            iids.sort_unstable();
                        }
                    }
                }
                self.snapshots = capture.then(|| Box::new(snapshots));
                self.rows.clear();
                self.locations.clear();
                self.resolved.clear();
            }
            return self.body.as_mut().expect("prepared body").next();
        }
        if let Some((plan, ordinals)) = self.lookups.pop_front() {
            return Ok(Some(WriteStep {
                plan,
                complete: false,
                ordinal: 0,
                expected_iid: None,
                missing_iid_noop: false,
                candidate_filter: false,
                reference_guard: false,
                reference_resolution: Some(Box::new(ResolutionStep {
                    ordinals,
                    guard: None,
                })),
            }));
        }
        if self.body.is_none() {
            let capture = self.kind == WriteKind::Update
                && self.rows.iter().any(|row| {
                    row.players.iter().any(|player| {
                        player.key.is_some() && player.candidates.contains(&self.shape.type_id)
                    })
                });
            let mut snapshots = ReferenceSnapshots::new();
            for (index, (row, player)) in self.locations.iter().copied().enumerate() {
                let iid = self.resolved[index]
                    .as_ref()
                    .ok_or_else(identity_error)?
                    .clone();
                let player = &mut self.rows[row].players[player];
                if capture {
                    snapshots
                        .entry(row)
                        .or_default()
                        .entry(player.role.clone())
                        .or_default()
                        .push(iid.to_ascii_lowercase());
                }
                player.iid = Some(iid);
                player.key = None;
            }
            let body = body(&self.authority, self.kind, &self.shape, &self.rows, None)?
                .prepare_static()?;
            if body.statement_count() > self.body_ceiling {
                return Err(identity_error());
            }
            for roles in snapshots.values_mut() {
                for iids in roles.values_mut() {
                    iids.sort_unstable();
                }
            }
            self.snapshots = capture.then(|| Box::new(snapshots));
            self.body = Some(body);
            // The body owns its input; release the wide staging copy.
            self.rows.clear();
            self.locations.clear();
            self.resolved.clear();
        }
        self.body.as_mut().ok_or_else(identity_error)?.next()
    }
    pub(super) fn accept(
        &mut self,
        step: &WriteStep,
        answers: Vec<(usize, String)>,
    ) -> Result<(), Diagnostic> {
        let expected = &step
            .reference_resolution
            .as_ref()
            .ok_or_else(identity_error)?
            .ordinals;
        if expected.len() != answers.len() {
            return Err(identity_error());
        }
        let mut seen = std::collections::BTreeSet::new();
        for (ordinal, iid) in answers {
            if expected.binary_search(&ordinal).is_err() || !seen.insert(ordinal) {
                return Err(identity_error());
            }
            step.check_identity(&iid)?;
            let (row, player) = *self.locations.get(ordinal).ok_or_else(identity_error)?;
            if self.rows[row].players[player]
                .iid
                .as_ref()
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(&iid))
            {
                return Err(identity_error());
            }
            let target = self.resolved.get_mut(ordinal).ok_or_else(identity_error)?;
            if target.is_some() {
                return Err(identity_error());
            }
            *target = Some(iid);
        }
        Ok(())
    }
    pub(super) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: Option<String>,
    ) -> Result<(), Diagnostic> {
        self.body
            .as_mut()
            .ok_or_else(identity_error)?
            .accept_identity(step, iid)
    }
    pub(super) fn take_snapshots(&mut self) -> Option<Box<ReferenceSnapshots>> {
        self.snapshots.take()
    }
}

fn player_references(players: &[Player]) -> Result<Vec<(RoleId, Reference<'_>)>, Diagnostic> {
    players
        .iter()
        .map(|player| {
            let identity = if let Some(iid) = &player.iid {
                ReferenceIdentity::Iid(iid)
            } else {
                let (attribute, value) = player.key.as_ref().ok_or_else(identity_error)?;
                ReferenceIdentity::Key {
                    attribute,
                    value: value.clone(),
                }
            };
            Ok((
                player.role.clone(),
                Reference {
                    candidates: &player.candidates,
                    identity,
                },
            ))
        })
        .collect()
}

fn body(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: &OwnedShape,
    owned: &[Row],
    replacements: Option<&[Vec<Player>]>,
) -> Result<WriteProgram, Diagnostic> {
    let players = owned
        .iter()
        .enumerate()
        .map(|(index, row)| {
            player_references(
                replacements.map_or(row.players.as_slice(), |players| players[index].as_slice()),
            )
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let rows = owned
        .iter()
        .zip(&players)
        .map(|(row, players)| WriteRow {
            iid: row.iid.as_deref(),
            fields: &row.fields,
            players,
        })
        .collect::<Vec<_>>();
    match build_many(authority, kind, shape.borrowed(), &rows, true, 0) {
        Ok(plans) => Ok(WriteProgram::Compact(plans)),
        Err(error) if forces_pieces(&error) => {
            split_resolved(authority, kind, shape.borrowed(), &rows)
        }
        Err(error) => Err(error),
    }
}

fn lookup(
    authority: &PlanAuthority,
    route: &LookupRoute,
    rows: Vec<InputRow>,
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = authority.draft(ValueTransport::Given);
    let root = draft.bind("root")?;
    let ordinal = draft.bind("ordinal")?;
    let position = draft.input("reference_ordinal", ValueTypeTag::Long, false)?;
    let domain = rows[0].values()[1]
        .as_ref()
        .ok_or_else(identity_error)?
        .value_type();
    let identity = draft.input("reference_identity", domain, false)?;
    let branches = route
        .candidates
        .iter()
        .map(|candidate| {
            reference_patterns(&mut draft, root, candidate, route.key.as_ref(), &identity)
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    draft.finish_rows(
        authority,
        vec![
            Stage::Match {
                patterns: vec![
                    Pattern::Let {
                        binding: ordinal,
                        expr: Expr::Operand(position),
                    },
                    QueryPattern::Or { branches }.into(),
                ],
            },
            Stage::Select {
                bindings: vec![root, ordinal],
            },
            Stage::Distinct,
        ],
        Operation::Rows,
        QueryOutput::Rows {
            columns: vec![ordinal, root],
        },
        Some(rows),
    )
}

pub(in crate::plan_builders::bulk_write) fn resolve_references(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<WriteProgram, Diagnostic> {
    let mut routes = BTreeMap::<LookupRoute, Vec<InputRow>>::new();
    let mut locations = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        for (player_index, (_, reference)) in row.players.iter().enumerate() {
            let ordinal = locations.len();
            locations.push((row_index, player_index));
            let (key, value) = match &reference.identity {
                ReferenceIdentity::Iid(value) => (None, super::iid(value)?),
                ReferenceIdentity::Key { attribute, value } => {
                    (Some((*attribute).clone()), value.clone())
                }
            };
            routes
                .entry(LookupRoute {
                    candidates: reference.candidates.to_vec(),
                    key,
                })
                .or_default()
                .push(InputRow::new(vec![
                    Some(CanonicalValue::Long(
                        i64::try_from(ordinal).map_err(|_| identity_error())?,
                    )),
                    Some(value),
                ]));
        }
    }
    let mut lookups = VecDeque::new();
    for (route, rows) in routes {
        let chunks = crate::plan_builders::input::chunks(rows, 0, crate::plan_builders::input::canonical_upper_bound, |row| usize::try_from(crate::projected_batch_executor::given_rows_bytes(row)?).map_err(|_| type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure()), |_, _| false).map_err(|error| invalid(error.code().as_str(), error.message().as_str()))?;
        for rows in chunks {
            let ordinals = rows
                .iter()
                .map(|row| match row.values()[0].as_ref() {
                    Some(CanonicalValue::Long(ordinal)) => {
                        usize::try_from(*ordinal).map_err(|_| identity_error())
                    }
                    _ => Err(identity_error()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            lookups.push_back((lookup(authority, &route, rows)?, ordinals));
        }
    }
    let shape = OwnedShape::of(&shape);
    let owned = rows.iter().map(Row::of).collect::<Vec<_>>();
    let mut maximal = owned
        .iter()
        .map(|row| row.players.clone())
        .collect::<Vec<_>>();
    // Real resolved IIDs can only shrink these canonical input cuts.
    let placeholder = format!(
        "0x{}",
        "0".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS)
    );
    for players in &mut maximal {
        for player in players {
            player.iid = Some(placeholder.clone());
            player.key = None;
        }
    }
    let body_ceiling = body(authority, kind, &shape, &owned, Some(&maximal))?.statement_count();
    let statements = lookups
        .len()
        .checked_add(body_ceiling)
        .ok_or_else(identity_error)?;
    let count = locations.len();
    Ok(WriteProgram::References(Box::new(ReferenceProgram {
        authority: authority.clone(),
        shape,
        kind,
        rows: owned,
        locations,
        resolved: vec![None; count],
        lookups,
        body: None,
        snapshots: None,
        statements,
        body_ceiling,
        frozen: false,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents};

    fn program(by_iid: bool) -> WriteProgram {
        let yaml = "format: typebridge.schema/v2\nattributes:\n  k: { value: string }\n  t: { value: string }\nrelations:\n  r:\n    owns:\n      k: { key: true }\n      t: { card: { min: 0 } }\n    relates:\n      prior: { card: { min: 0 } }\nplays:\n  r:\n    r: [prior]\n";
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("snapshot.yaml").unwrap(), yaml)]).unwrap(),
        )
        .unwrap();
        let authority = PlanAuthority::new(
            declared,
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"reference-snapshot",
            )
            .unwrap(),
        );
        let relation = TypeId::new(TypeKind::Relation, "r").unwrap();
        let key_id = AttributeId::new("k").unwrap();
        let key = OwnsFactId::new(relation.clone(), key_id.clone()).unwrap();
        let tag = OwnsFactId::new(relation.clone(), AttributeId::new("t").unwrap()).unwrap();
        let role = RoleId::new("r", "prior").unwrap();
        let text = |value: &str| CanonicalValue::String(CanonicalString::new(value).unwrap());
        let mut fields = vec![(key.clone(), text("new-key"))];
        fields.extend((0..300).map(|index| (tag.clone(), text(&format!("v-{index}")))));
        let candidates = [relation.clone()];
        let identity = if by_iid {
            ReferenceIdentity::Iid("0x01")
        } else {
            ReferenceIdentity::Key {
                attribute: &key_id,
                value: text("old-key"),
            }
        };
        let players = [(
            role.clone(),
            Reference {
                candidates: &candidates,
                identity,
            },
        )];
        resolve_references(
            &authority,
            WriteKind::Update,
            WriteShape {
                references_see_prior_writes: false,
                single_input: false,
                type_id: &relation,
                keys: std::slice::from_ref(&key),
                cleared_fields: &[key.clone(), tag],
                cleared_roles: std::slice::from_ref(&role),
                document_roles: vec![DocumentRole::collected(role.clone())],
                include_subtypes: false,
                return_documents: true,
            },
            &[WriteRow {
                iid: Some("0x01"),
                fields: &fields,
                players: &players,
            }],
        )
        .unwrap()
    }

    #[test]
    fn reference_snapshots_reject_missing_repeated_and_foreign_ordinals_before_writes() {
        for answers in [
            vec![],
            vec![(1, "0x01".into())],
            vec![(0, "0x01".into()), (0, "0x02".into())],
        ] {
            let mut program = program(false);
            let step = program.next().unwrap().unwrap();
            assert!(step.reference_resolution.is_some());
            assert!(
                step.plan
                    .local()
                    .pipeline()
                    .iter()
                    .all(|stage| !stage.is_write())
            );
            assert_eq!(
                program
                    .accept_reference_iids(&step, answers)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "provider_identity_mismatch"
            );
            assert!(program.next().is_err());
        }
        let mut known = program(true);
        let step = known.next().unwrap().unwrap();
        assert_eq!(
            known
                .accept_reference_iids(&step, vec![(0, "0x02".into())])
                .unwrap_err()
                .code()
                .as_str(),
            "provider_identity_mismatch"
        );
    }

    #[test]
    fn reference_snapshots_pin_iids_and_drop_key_lookups_from_later_pieces() {
        let mut program = program(false);
        assert_eq!(program.statement_count(), 3);
        let step = program.next().unwrap().unwrap();
        assert!(
            String::from_utf8(step.plan.local().canonical_bytes().unwrap())
                .unwrap()
                .contains("old-key")
        );
        program
            .accept_reference_iids(&step, vec![(0, "0x01".into())])
            .unwrap();
        let mut writes = 0;
        while let Some(step) = program.next().unwrap() {
            assert!(
                !String::from_utf8(step.plan.local().canonical_bytes().unwrap())
                    .unwrap()
                    .contains("old-key")
            );
            assert!(step.reference_resolution.is_none());
            assert!(!step.reference_guard);
            writes += 1;
            if !step.complete {
                program.accept_identity(&step, Some("0x01".into())).unwrap();
            }
        }
        assert_eq!(writes, 2);
        let snapshots = program.take_reference_snapshots().unwrap();
        assert_eq!(
            snapshots[&0][&RoleId::new("r", "prior").unwrap()],
            vec![String::from("0x01")]
        );
    }

    #[test]
    fn reference_snapshot_rows_reject_a_thing_outside_the_lookup_domain() {
        let mut program = program(false);
        let step = program.next().unwrap().unwrap();
        let answer = PlanAnswer::Rows(vec![
            serde_json::json!({"ordinal1":{"kind":"value","value":CanonicalValue::Long(0)},"root0":{"kind":"thing","iid":"0x01","type_id":TypeId::new(TypeKind::Entity,"wrong").unwrap()}}),
        ]);
        assert!(step.reference_iids(&answer).is_err());
    }
    #[test]
    fn insert_and_keyed_put_prefix_proofs_preserve_allocation_dependencies() {
        for kind in [WriteKind::Insert, WriteKind::Put] {
            let WriteProgram::References(mut owner) = program(false) else {
                panic!("references")
            };
            owner.kind = kind;
            owner.rows[0].iid = None;
            let mut second = owner.rows[0].clone();
            second
                .fields
                .retain(|(field, _)| owner.shape.keys.contains(field));
            second.fields[0].1 =
                CanonicalValue::String(CanonicalString::new("second-key").unwrap());
            second.players.push(second.players[0].clone());
            second.players[1].key.as_mut().unwrap().1 =
                CanonicalValue::String(CanonicalString::new("other-key").unwrap());
            owner.rows.push(second);
            owner.locations = vec![(0, 0), (1, 0), (1, 1)];
            owner.resolved = vec![None; 3];
            let mut limits = type_bridge_contract::limits::QueryLimits::V3;
            limits.structural.selected_slots = 4;
            owner.authority = owner
                .authority
                .with_query_limits(std::sync::Arc::new(limits));
            let authority = owner.authority.clone();
            let program = WriteProgram::References(owner).prepare_static().unwrap();
            let entries = program
                .staged_entries(&authority)
                .unwrap()
                .expect("closed presence proofs");
            assert_eq!(entries.len(), 5);
            assert!(entries[..2].iter().all(|entry| !entry.plan().writes()));
            assert!(entries[2..].iter().all(|entry| entry.plan().writes()));
            assert!(
                entries[2]
                    .result_refs()
                    .iter()
                    .all(|reference| reference.source_plan == 1)
            );
            assert_eq!(entries[3].result_refs().len(), 1);
            assert_eq!(entries[3].result_refs()[0].source_plan, 2);
            assert_eq!(entries[3].result_refs()[0].source_column, 1);
            assert_eq!(program.guard_contexts().len(), 3);
            assert!(entries[2..].iter().any(|entry| {
                String::from_utf8(entry.plan().canonical_bytes().unwrap())
                    .unwrap()
                    .contains("old-key")
            }));
        }
    }

    #[test]
    fn oversized_snapshot_tuples_chain_before_writes_and_split_sources() {
        use type_bridge_contract::query_remote::RemoteValue;
        let WriteProgram::References(mut owner) = program(false) else {
            panic!("references")
        };
        let mut limits = type_bridge_contract::limits::QueryLimits::V3;
        limits.structural.selected_slots = 4;
        owner.authority = owner
            .authority
            .with_query_limits(std::sync::Arc::new(limits));
        let original = owner.rows[0].players[0].clone();
        owner.rows[0].players = (0..6)
            .map(|index| {
                let mut player = original.clone();
                player.key.as_mut().unwrap().1 = CanonicalValue::String(
                    CanonicalString::new(format!("old-key-{index}")).unwrap(),
                );
                player
            })
            .collect();
        owner.locations = (0..6).map(|player| (0, player)).collect();
        owner.resolved = vec![None; 6];
        let authority = owner.authority.clone();
        let mut program = WriteProgram::References(owner).prepare_static().unwrap();
        let entries = program
            .staged_entries(&authority)
            .unwrap()
            .expect("bounded snapshots");
        let lookups = entries
            .iter()
            .take_while(|entry| !entry.plan().writes())
            .count();
        assert_eq!(lookups, 4);
        for (index, entry) in entries.iter().take(lookups).enumerate() {
            assert!(!entry.plan().writes());
            assert_eq!(
                entry.result_refs().len(),
                if index == 0 {
                    0
                } else {
                    let QueryOutput::Rows { columns } = entries[index - 1].plan().output() else {
                        panic!("snapshot tuple")
                    };
                    columns.len() / 2
                }
            );
            assert!(
                entry
                    .result_refs()
                    .iter()
                    .all(|reference| reference.source_plan as usize + 1 == index)
            );
        }
        assert!(
            entries[lookups]
                .result_refs()
                .iter()
                .all(|reference| reference.source_plan as usize == lookups - 1)
        );
        let mut referenced = std::collections::BTreeSet::new();
        for entry in &entries[lookups..] {
            let sources = entry
                .result_refs()
                .iter()
                .map(|reference| reference.source_plan)
                .collect::<std::collections::BTreeSet<_>>();
            assert!(sources.len() <= 1);
            referenced.extend(sources);
        }
        assert_eq!(referenced, (0..4).collect());
        let model = TypeId::new(TypeKind::Relation, "r").unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..lookups {
            let step = program.next().unwrap().unwrap();
            assert!(!step.plan.local().writes());
            let ordinals = step.reference_resolution.as_ref().unwrap().ordinals.clone();
            let mut cells = Vec::new();
            let mut answers = Vec::new();
            for ordinal in ordinals {
                seen.insert(ordinal);
                let iid = format!("0x{:02x}", ordinal + 2);
                cells.push(RemoteValue::Value {
                    value: CanonicalValue::Long(ordinal as i64),
                });
                cells.push(RemoteValue::Thing {
                    iid: iid.clone(),
                    type_id: model.clone(),
                });
                answers.push((ordinal, iid));
            }
            let answer = PlanAnswer::from_staged_v3(
                &StagedAnswerV3::Rows { rows: vec![cells] },
                step.plan.local(),
            )
            .unwrap();
            program.record_answer(&step, &answer).unwrap();
            program.accept_reference_iids(&step, answers).unwrap();
        }
        assert_eq!(seen, (0..6).collect());
        let seed = program.next().unwrap().unwrap();
        assert!(seed.plan.local().writes());
        seed.check_identity("0x01").unwrap();
        assert!(seed.check_identity("0x02").is_err());
        assert_eq!(
            program.take_reference_snapshots().unwrap()[&0][&RoleId::new("r", "prior").unwrap()],
            (2..8)
                .map(|index| format!("0x{index:02x}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn frozen_reference_graph_keeps_authored_targets_and_source_snapshots() {
        use type_bridge_contract::query_remote::RemoteValue;
        let mut program = program(false).prepare_static().unwrap();
        let authority = match &program {
            WriteProgram::References(program) => program.authority.clone(),
            _ => panic!("reference owner"),
        };
        let entries = program
            .staged_entries(&authority)
            .unwrap()
            .expect("a frozen proxy graph");
        assert_eq!(entries.len(), 3);
        assert!(!entries[0].plan().writes());
        assert!(
            entries[1..]
                .iter()
                .flat_map(|entry| entry.result_refs())
                .all(|reference| reference.source_plan == 0)
        );
        let lookup = program.next().unwrap().unwrap();
        let model = TypeId::new(TypeKind::Relation, "r").unwrap();
        let answer = PlanAnswer::from_staged_v3(
            &StagedAnswerV3::Rows {
                rows: vec![vec![
                    RemoteValue::Value {
                        value: CanonicalValue::Long(0),
                    },
                    RemoteValue::Thing {
                        iid: "0x02".into(),
                        type_id: model.clone(),
                    },
                ]],
            },
            lookup.plan.local(),
        )
        .unwrap();
        program.record_answer(&lookup, &answer).unwrap();
        program
            .accept_reference_iids(&lookup, vec![(0, "0x02".into())])
            .unwrap();
        let seed = program.next().unwrap().unwrap();
        seed.check_identity("0x01").unwrap();
        assert!(seed.check_identity("0x02").is_err());
        let plan = String::from_utf8(seed.plan.local().canonical_bytes().unwrap()).unwrap();
        assert!(!plan.contains("old-key"));
        assert!(plan.contains("new-key"));
        let snapshot = program.take_reference_snapshots().unwrap();
        assert_eq!(
            snapshot[&0][&RoleId::new("r", "prior").unwrap()],
            vec!["0x02".to_owned()]
        );
        let answer = PlanAnswer::from_staged_v3(
            &StagedAnswerV3::Rows {
                rows: vec![vec![
                    RemoteValue::Value {
                        value: CanonicalValue::Long(0),
                    },
                    RemoteValue::Thing {
                        iid: "0x01".into(),
                        type_id: model,
                    },
                ]],
            },
            seed.plan.local(),
        )
        .unwrap();
        program.record_answer(&seed, &answer).unwrap();
        let final_step = program.next().unwrap().unwrap();
        assert!(final_step.complete);
        assert!(
            final_step
                .plan
                .local()
                .inputs()
                .iter()
                .enumerate()
                .filter(|(_, input)| input.public_name().as_str() == "target_iid")
                .all(|(column, _)| final_step
                    .plan
                    .local()
                    .given_rows()
                    .unwrap()
                    .iter()
                    .all(|row| row.values()[column] == Some(super::super::iid("0x01").unwrap())))
        );
        assert!(program.next().unwrap().is_none());
    }
}
