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
    let mut guards = Vec::new();
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
            let selected = references
                .iter()
                .filter(|reference| ordinals.binary_search(&reference.0).is_ok())
                .cloned()
                .collect();
            guards.push(WriteStep {
                plan: lookup(authority, &route, rows)?,
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
        }
    }
    Ok(guards)
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
}
impl ReferenceProgram {
    pub(super) fn statement_count(&self) -> usize {
        self.statements
    }
    pub(super) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
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
            let body = body(&self.authority, self.kind, &self.shape, &self.rows, None)?;
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
            replacements
                .map_or(row.players.as_slice(), |players| players[index].as_slice())
                .iter()
                .map(|player| {
                    Ok((
                        player.role.clone(),
                        Reference {
                            candidates: &player.candidates,
                            identity: ReferenceIdentity::Iid(
                                player.iid.as_deref().ok_or_else(identity_error)?,
                            ),
                        },
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()
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
        let yaml = "format: typebridge.schema/v2\nattributes:\n  k: { value: string }\n  t: { value: string }\nrelations:\n  r:\n    owns:\n      k: { key: true }\n      t: { card: { min: 0 } }\n    relates:\n      prior: { card: { min: 0, max: 1 } }\nplays:\n  r:\n    r: [prior]\n";
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
}
