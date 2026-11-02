//! Preserve per-row Put visibility in one straight-line native pipeline.
use super::*;
use type_bridge_contract::migration_assertion::BindingId;

pub(super) fn requires_sequence(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: &WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<bool, Diagnostic> {
    if rows.len() < 2 {
        return Ok(false);
    }
    if matches!(kind, WriteKind::Insert | WriteKind::Put) {
        if rows.iter().any(|row| {
            row.players
                .iter()
                .any(|(_, reference)| reference.candidates.contains(shape.type_id))
        }) {
            return Ok(true);
        }
        if kind == WriteKind::Put && !shape.keys.is_empty() {
            let mut identities = std::collections::BTreeSet::new();
            for row in rows {
                let keys = row
                    .fields
                    .iter()
                    .filter(|(field, _)| shape.keys.contains(field))
                    .collect::<Vec<_>>();
                if !identities.insert(type_bridge_contract::codec::to_canonical_json(&keys)?) {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    if kind == WriteKind::PartialUpdate {
        if rows.iter().all(|row| {
            row.fields
                .iter()
                .all(|(field, _)| shape.keys.contains(field))
        }) {
            return Ok(false);
        }
        if changes_unique(authority, shape, rows) {
            return Ok(true);
        }
        if rows.iter().all(|row| row.iid.is_some()) {
            return Ok(rows
                .iter()
                .map(|row| row.iid.expect("checked IID").to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len());
        }
        if rows.iter().any(|row| row.iid.is_some()) {
            return Ok(true);
        }
    }
    let mut first_shape = None;
    let mut identities = std::collections::BTreeSet::new();
    let mut first_keys = None;
    for row in rows {
        if kind == WriteKind::PartialPut
            && changes_unique(authority, shape, std::slice::from_ref(row))
        {
            return Ok(true);
        }
        let mut fields = BTreeMap::<_, Vec<Vec<u8>>>::new();
        for (field, value) in row.fields {
            fields
                .entry(field.clone())
                .or_default()
                .push(type_bridge_contract::codec::to_canonical_json(value)?);
        }
        for values in fields.values_mut() {
            values.sort();
            if values.windows(2).any(|pair| pair[0] == pair[1]) {
                return Ok(true);
            }
        }
        let mut players = BTreeMap::<_, Vec<Vec<u8>>>::new();
        for (role, reference) in row.players {
            if reference
                .candidates
                .iter()
                .any(|candidate| candidate == shape.type_id)
            {
                return Ok(true);
            }
            let (route, value) = player_value(role, reference)?;
            players
                .entry(route)
                .or_default()
                .push(type_bridge_contract::codec::to_canonical_json(&value)?);
        }
        for values in players.values_mut() {
            values.sort();
        }
        let row_shape = (
            fields
                .iter()
                .map(|(field, values)| (field.clone(), values.len()))
                .collect::<Vec<_>>(),
            players
                .iter()
                .map(|(route, values)| (route.clone(), values.len()))
                .collect::<Vec<_>>(),
        );
        if kind == WriteKind::MatchPut {
            if first_shape
                .as_ref()
                .is_some_and(|first| first != &row_shape)
            {
                return Ok(true);
            }
            first_shape = Some(row_shape);
        }
        let keys = fields
            .iter()
            .filter(|(field, _)| shape.keys.contains(field))
            .map(|(field, values)| (field.clone(), values.clone()))
            .collect::<Vec<_>>();
        if matches!(kind, WriteKind::PartialPut | WriteKind::PartialUpdate) {
            let key_shape = keys
                .iter()
                .map(|(field, _)| field.clone())
                .collect::<Vec<_>>();
            if first_keys.as_ref().is_some_and(|first| first != &key_shape) {
                return Ok(true);
            }
            first_keys = Some(key_shape);
        }
        let identity =
            if !keys.is_empty() {
                type_bridge_contract::codec::to_canonical_json(&keys)?
            } else {
                if row.players.iter().any(|(_, reference)| {
                    matches!(reference.identity, ReferenceIdentity::Key { .. })
                }) {
                    return Ok(true);
                }
                type_bridge_contract::codec::to_canonical_json(&(
                    fields.into_iter().collect::<Vec<_>>(),
                    players.values().collect::<Vec<_>>(),
                ))?
            };
        if !identities.insert(identity) {
            return Ok(true);
        }
    }
    debug_assert!(matches!(
        kind,
        WriteKind::PartialPut
            | WriteKind::MatchPut
            | WriteKind::PartialUpdate
            | WriteKind::Insert
            | WriteKind::Put
    ));
    Ok(false)
}

fn changes_unique(
    authority: &PlanAuthority,
    shape: &WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> bool {
    use type_bridge_contract::schema::{
        AnnotationFactId, AnnotationKindId, AnnotationSubjectId, SchemaFactId,
    };
    rows.iter().any(|row| {
        row.fields.iter().any(|(field, _)| {
            !shape.keys.contains(field)
                && authority
                    .declared()
                    .fact(&SchemaFactId::Annotation(AnnotationFactId::new(
                        AnnotationSubjectId::Owns(field.clone()),
                        AnnotationKindId::Unique,
                    )))
                    .is_some()
        })
    })
}

fn value_alias(
    draft: &mut PlanDraft,
    patterns: &mut Vec<Pattern>,
    retained: &mut Vec<BindingId>,
    value: CanonicalValue,
) -> Result<QueryOperand, Diagnostic> {
    let binding = draft.bind("value")?;
    patterns.push(Pattern::Let {
        binding,
        expr: Expr::Operand(draft.value(value)?),
    });
    retained.push(binding);
    Ok(QueryOperand::Binding { binding })
}

fn one(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: &WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<BuiltPlan, Diagnostic> {
    let mut draft = PlanDraft::new(ValueTransport::Given);
    let mut initial = Vec::new();
    let mut retained = Vec::new();
    let mut prepared = Vec::new();
    for row in rows {
        let root = draft.bind("row")?;
        let fields = row
            .fields
            .iter()
            .map(|(field, value)| {
                Ok((
                    field.clone(),
                    value_alias(&mut draft, &mut initial, &mut retained, value.clone())?,
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let mut patterns = Vec::new();
        let mut players = Vec::new();
        let mut player_iids: Vec<(RoleId, BindingId)> = Vec::new();
        for (role, reference) in row.players {
            let player = draft.bind("player")?;
            let (key, input) = match &reference.identity {
                ReferenceIdentity::Iid(_) => (None, None),
                ReferenceIdentity::Key { attribute, value } => (
                    Some(*attribute),
                    Some(value_alias(
                        &mut draft,
                        &mut initial,
                        &mut retained,
                        value.clone(),
                    )?),
                ),
            };
            let mut branches = Vec::new();
            for candidate in reference.candidates {
                let mut branch = vec![QueryPattern::Isa {
                    binding: player,
                    type_id: candidate.clone(),
                    include_subtypes: false,
                }];
                if let (Some(attribute), Some(input)) = (key, &input) {
                    let binding = draft.bind("key")?;
                    branch.extend([
                        QueryPattern::Has {
                            attribute: binding,
                            owner: player,
                            attribute_id: attribute.clone(),
                        },
                        QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding { binding },
                            right: input.clone(),
                        },
                    ]);
                }
                branches.push(branch);
            }
            patterns.push(QueryPattern::Or { branches }.into());
            if let ReferenceIdentity::Iid(iid) = reference.identity {
                patterns.push(Pattern::Iid {
                    binding: player,
                    iid: iid.into(),
                });
            }
            if row
                .players
                .iter()
                .filter(|(candidate, _)| candidate == role)
                .take(2)
                .count()
                > 1
            {
                let binding = draft.bind("player_iid")?;
                patterns.push(Pattern::Let {
                    binding,
                    expr: Expr::Iid { binding: player },
                });
                for (previous_role, previous) in &player_iids {
                    if previous_role == role {
                        patterns.push(
                            QueryPattern::Value {
                                comparator: ValueComparator::NotEqual,
                                left: QueryOperand::Binding { binding: *previous },
                                right: QueryOperand::Binding { binding },
                            }
                            .into(),
                        );
                    }
                }
                player_iids.push((role.clone(), binding));
            }
            players.push((role.clone(), player));
        }
        if kind == WriteKind::PartialUpdate {
            let mut lookup = vec![QueryPattern::Isa {
                binding: root,
                type_id: shape.type_id.clone(),
                include_subtypes: true,
            }];
            if let Some(iid) = row.iid {
                let input = draft.value(super::iid(iid)?)?;
                let alias = draft.bind("value")?;
                initial.push(Pattern::Let {
                    binding: alias,
                    expr: Expr::Operand(input.clone()),
                });
                retained.push(alias);
                lookup.push(QueryPattern::Iid {
                    binding: root,
                    iid: input,
                });
            } else {
                for (field, operand) in &fields {
                    if shape.keys.contains(field) {
                        let attribute = draft.bind("identity_key")?;
                        lookup.extend([
                            QueryPattern::Has {
                                attribute,
                                owner: root,
                                attribute_id: field.attribute().clone(),
                            },
                            QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: QueryOperand::Binding { binding: attribute },
                                right: operand.clone(),
                            },
                        ]);
                    }
                }
                if lookup.len() == 1 {
                    return Err(invalid(
                        "manager_identity_missing",
                        "Mutation requires an IID or key attributes",
                    ));
                }
            }
            patterns.push(QueryPattern::Try { patterns: lookup }.into());
        }
        prepared.push((root, fields, players, patterns));
    }
    let mut pipeline = if initial.is_empty() {
        Vec::new()
    } else {
        vec![Stage::Match { patterns: initial }]
    };
    let mut outputs = Vec::new();
    for (root, fields, players, patterns) in prepared {
        if !patterns.is_empty() {
            if outputs.is_empty() {
                if let [Stage::Match { patterns: first }] = pipeline.as_mut_slice() {
                    first.extend(patterns);
                } else {
                    pipeline.push(Stage::Match { patterns });
                }
            } else {
                pipeline.push(Stage::Match { patterns });
            }
        }
        if matches!(kind, WriteKind::Insert | WriteKind::Put) {
            let keyed_put = kind == WriteKind::Put && !shape.keys.is_empty();
            if keyed_put {
                let (keys, attached): (Vec<_>, Vec<_>) = fields
                    .into_iter()
                    .partition(|(field, _)| shape.keys.contains(field));
                pipeline.push(Stage::Put {
                    include_subtypes: false,
                    things: vec![InsertThing {
                        binding: root,
                        type_id: shape.type_id.clone(),
                        fields: keys,
                        players: vec![],
                    }],
                });
                let deleted = shape
                    .cleared_fields
                    .iter()
                    .filter(|field| !shape.keys.contains(field))
                    .map(|field| DeleteItem::Has {
                        owner: root,
                        field: field.clone(),
                    })
                    .chain(shape.cleared_roles.iter().map(|role| DeleteItem::Players {
                        relation: root,
                        role: role.clone(),
                    }))
                    .collect::<Vec<_>>();
                if !deleted.is_empty() {
                    pipeline.push(Stage::Delete { items: deleted });
                    let mut visible = retained.clone();
                    visible.push(root);
                    visible.extend(players.iter().map(|(_, player)| *player));
                    visible.sort_unstable();
                    visible.dedup();
                    pipeline.extend([Stage::Select { bindings: visible }, Stage::Distinct]);
                }
                let items = attached
                    .into_iter()
                    .map(|(field, operand)| AttachItem::Has {
                        optional: false,
                        owner: root,
                        field,
                        operand,
                    })
                    .chain(players.into_iter().map(|(role, player)| AttachItem::Links {
                        optional: false,
                        relation: root,
                        role,
                        player,
                    }))
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    pipeline.push(Stage::Attach { items });
                }
            } else {
                pipeline.push(Stage::Insert {
                    things: vec![InsertThing {
                        binding: root,
                        type_id: shape.type_id.clone(),
                        fields,
                        players,
                    }],
                });
            }
        } else {
            let keyed_partial = kind == WriteKind::PartialPut
                && fields.iter().any(|(field, _)| shape.keys.contains(field));
            let (seed, attached): (Vec<_>, Vec<_>) = fields.into_iter().partition(|(field, _)| {
                !(keyed_partial || kind == WriteKind::PartialUpdate) || shape.keys.contains(field)
            });
            if kind != WriteKind::PartialUpdate {
                pipeline.push(Stage::Put {
                    include_subtypes: true,
                    things: vec![InsertThing {
                        binding: root,
                        type_id: shape.type_id.clone(),
                        fields: seed,
                        players,
                    }],
                });
            }
            if keyed_partial || kind == WriteKind::PartialUpdate {
                let cleared = attached
                    .iter()
                    .map(|(field, _)| field.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                if !cleared.is_empty() {
                    pipeline.push(Stage::Delete {
                        items: cleared
                            .into_iter()
                            .map(|field| DeleteItem::Has { owner: root, field })
                            .collect(),
                    });
                }
                if !attached.is_empty() {
                    pipeline.push(Stage::Attach {
                        items: attached
                            .into_iter()
                            .map(|(field, operand)| AttachItem::Has {
                                optional: kind == WriteKind::PartialUpdate,
                                owner: root,
                                field,
                                operand,
                            })
                            .collect(),
                    });
                }
            }
        }
        retained.push(root);
        retained.sort_unstable();
        outputs.push(root);
        pipeline.extend([
            Stage::Select {
                bindings: retained.clone(),
            },
            Stage::Distinct,
        ]);
    }
    // Inputs with no fields/players need no given row; the first Put allocates.
    pipeline.truncate(pipeline.len() - 2);
    pipeline.extend([
        Stage::Select {
            bindings: outputs.clone(),
        },
        Stage::Distinct,
    ]);
    let operation = if pipeline.iter().any(Stage::is_write) {
        Operation::Write {
            returns: if kind == WriteKind::PartialUpdate {
                WriteReturns::Matched
            } else {
                WriteReturns::Rows
            },
        }
    } else {
        Operation::Count
    };
    draft.finish(
        authority,
        pipeline,
        operation,
        QueryOutput::Rows {
            columns: if kind == WriteKind::PartialUpdate {
                vec![]
            } else {
                outputs
            },
        },
    )
}

pub(super) fn build(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<WriteProgram, Diagnostic> {
    let mut plans = Vec::new();
    let mut offset = 0;
    while offset < rows.len() {
        let mut high = rows.len() - offset;
        let mut low = 0;
        while low < high {
            let count = low + (high - low).div_ceil(2);
            match one(authority, kind, &shape, &rows[offset..offset + count]) {
                Ok(_) => low = count,
                Err(error) if program::forces_pieces(&error) => high = count - 1,
                Err(error) => return Err(error),
            }
        }
        if low == 0 {
            return one(authority, kind, &shape, &rows[offset..offset + 1])
                .map(|_| unreachable!("one row failed its canonical fit check"));
        }
        let plan = one(authority, kind, &shape, &rows[offset..offset + low])?;
        plans.push((
            plan,
            if kind == WriteKind::PartialUpdate {
                vec![]
            } else {
                (offset..offset + low).collect()
            },
        ));
        offset += low;
    }
    Ok(WriteProgram::Sequential(plans))
}
