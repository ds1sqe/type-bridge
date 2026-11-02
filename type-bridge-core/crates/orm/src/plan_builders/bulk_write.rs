//! One nullable input matrix for complete manager writes.

use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, DocumentRole, Expr, InsertThing, Operation, Pattern, Stage,
    ThingDocument, WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputRow, QueryOperand, QueryOutput, QueryPattern, ReduceAssignment, Reducer,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};

use super::authority::PlanAuthority;
use super::reference::{Reference, ReferenceIdentity};
use super::{BuiltPlan, PlanDraft, ValueTransport};

#[path = "bulk_write_program.rs"]
mod program;
#[path = "bulk_write_sequence.rs"]
mod sequence;
pub(crate) use program::{IdentityLayout, ReferenceSnapshots, WriteProgram, forces_pieces};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteKind {
    Insert,
    Put,
    Update,
    Attach,
    PartialUpdate,
    PartialPut,
    MatchPut,
    Delete,
    DeleteReturning,
}

pub(crate) struct WriteRow<'a> {
    pub(crate) iid: Option<&'a str>,
    pub(crate) fields: &'a [(OwnsFactId, CanonicalValue)],
    pub(crate) players: &'a [(RoleId, Reference<'a>)],
}

#[derive(Clone)]
pub(crate) struct WriteShape<'a> {
    pub(crate) type_id: &'a TypeId,
    pub(crate) keys: &'a [OwnsFactId],
    pub(crate) cleared_fields: &'a [OwnsFactId],
    pub(crate) cleared_roles: &'a [RoleId],
    pub(crate) document_roles: Vec<DocumentRole>,
    pub(crate) include_subtypes: bool,
    pub(crate) return_documents: bool,
    /// Generic managers resolve players after earlier rows; explicit batches resolve at call start.
    pub(crate) references_see_prior_writes: bool,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlayerRoute {
    role: RoleId,
    candidates: Vec<TypeId>,
    key: Option<AttributeId>,
}

fn invalid(code: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static code"),
        message,
    )
}

fn iid(value: &str) -> Result<CanonicalValue, Diagnostic> {
    if !type_bridge_contract::id::is_canonical_thing_iid(value) {
        return Err(invalid(
            "query_plan_invalid_iid",
            "A manager write requires canonical IIDs",
        ));
    }
    Ok(CanonicalValue::String(CanonicalString::new(
        value.to_ascii_lowercase(),
    )?))
}

fn player_value(
    role: &RoleId,
    reference: &Reference<'_>,
) -> Result<(PlayerRoute, CanonicalValue), Diagnostic> {
    let (key, value) = match &reference.identity {
        ReferenceIdentity::Iid(value) => (None, iid(value)?),
        ReferenceIdentity::Key { attribute, value } => (Some((*attribute).clone()), value.clone()),
    };
    Ok((
        PlayerRoute {
            role: role.clone(),
            candidates: reference.candidates.to_vec(),
            key,
        },
        value,
    ))
}

fn reference_patterns(
    draft: &mut PlanDraft,
    player: type_bridge_contract::migration_assertion::BindingId,
    candidate: &TypeId,
    key: Option<&AttributeId>,
    input: &QueryOperand,
) -> Result<Vec<QueryPattern>, Diagnostic> {
    let mut patterns = vec![QueryPattern::Isa {
        binding: player,
        type_id: candidate.clone(),
        include_subtypes: false,
    }];
    if let Some(attribute_id) = key {
        let value = draft.bind("key")?;
        patterns.push(QueryPattern::Has {
            attribute: value,
            owner: player,
            attribute_id: attribute_id.clone(),
        });
        patterns.push(QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left: QueryOperand::Binding { binding: value },
            right: input.clone(),
        });
    } else {
        patterns.push(QueryPattern::Iid {
            binding: player,
            iid: input.clone(),
        });
    }
    Ok(patterns)
}

fn present(input: &QueryOperand) -> QueryPattern {
    QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: input.clone(),
        right: input.clone(),
    }
}

fn identity_patterns(
    draft: &mut PlanDraft,
    root: type_bridge_contract::migration_assertion::BindingId,
    type_id: &TypeId,
    input: &QueryOperand,
    keys: &[(OwnsFactId, QueryOperand)],
) -> Result<Vec<QueryPattern>, Diagnostic> {
    let mut fallback = vec![QueryPattern::Not {
        patterns: vec![present(input)],
    }];
    for (field, input) in keys {
        let attribute = draft.bind("identity_key")?;
        fallback.push(QueryPattern::Or {
            branches: vec![
                vec![QueryPattern::Not {
                    patterns: vec![present(input)],
                }],
                vec![
                    QueryPattern::Has {
                        attribute,
                        owner: root,
                        attribute_id: field.attribute().clone(),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: attribute },
                        right: input.clone(),
                    },
                ],
            ],
        });
    }
    // The caller proves each IID-less row supplies at least one key.
    Ok(vec![
        QueryPattern::Isa {
            binding: root,
            type_id: type_id.clone(),
            include_subtypes: true,
        },
        QueryPattern::Or {
            branches: vec![
                vec![QueryPattern::Iid {
                    binding: root,
                    iid: input.clone(),
                }],
                fallback,
            ],
        },
    ])
}

/// Lower every input row to the same insert/put/update pipeline. Empty
/// nullable cells omit attachments; a supplied missing reference filters out
/// its entire row before any mutation, so answer correlation can refuse it.
pub(crate) fn build_many(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
    complete: bool,
    ordinal_offset: usize,
) -> Result<Vec<BuiltPlan>, Diagnostic> {
    if rows.is_empty() {
        return Err(invalid(
            "manager_empty_write",
            "A write plan requires at least one row",
        ));
    }
    let mut field_widths = BTreeMap::new();
    let mut player_widths = BTreeMap::new();
    let mut matrices = Vec::with_capacity(rows.len());
    for row in rows {
        if !matches!(
            kind,
            WriteKind::Attach
                | WriteKind::PartialUpdate
                | WriteKind::Delete
                | WriteKind::DeleteReturning
        ) && shape.type_id.kind() == TypeKind::Relation
            && row.players.is_empty()
        {
            return Err(invalid(
                "query_plan_insert_relation_players",
                "Every relation write row requires at least one player",
            ));
        }
        let mut fields = BTreeMap::<_, Vec<_>>::new();
        for (field, value) in row.fields {
            fields.entry(field.clone()).or_default().push(value.clone());
        }
        if matches!(kind, WriteKind::Put | WriteKind::PartialPut)
            && shape
                .keys
                .iter()
                .any(|key| fields.get(key).is_none_or(|values| values.len() != 1))
        {
            return Err(invalid(
                "manager_key_missing",
                "Every keyed put row requires each scalar key",
            ));
        }
        if matches!(
            kind,
            WriteKind::PartialUpdate | WriteKind::Delete | WriteKind::DeleteReturning
        ) && row.iid.is_none()
            && !shape.keys.iter().any(|key| fields.contains_key(key))
            && row.players.is_empty()
        {
            return Err(invalid(
                "manager_identity_missing",
                "Mutation requires an IID, key attributes, or role players",
            ));
        }
        let mut players = BTreeMap::<_, Vec<_>>::new();
        for (role, reference) in row.players {
            let (route, value) = player_value(role, reference)?;
            players.entry(route).or_default().push(value);
        }
        for (field, values) in &fields {
            let width = field_widths
                .entry(field.clone())
                .or_insert((0, values[0].value_type()));
            width.0 = width.0.max(values.len());
        }
        for (route, values) in &players {
            let width = player_widths
                .entry(route.clone())
                .or_insert((0, values[0].value_type()));
            width.0 = width.0.max(values.len());
        }
        matrices.push((fields, players));
    }
    let mut draft = PlanDraft::new(ValueTransport::Given);
    let root = draft.bind("root")?;
    let ordinal = draft.bind("ordinal")?;
    let fixed_columns = field_widths
        .values()
        .map(|(width, _)| *width)
        .sum::<usize>()
        + player_widths
            .values()
            .map(|(width, _)| *width)
            .sum::<usize>()
        + usize::from(matches!(
            kind,
            WriteKind::Update
                | WriteKind::Attach
                | WriteKind::PartialUpdate
                | WriteKind::Delete
                | WriteKind::DeleteReturning
        ));
    let constant_ordinal = !complete && fixed_columns >= type_bridge_contract::limits::MAX_BINDINGS;
    let ordinal_input = if constant_ordinal {
        QueryOperand::Literal {
            value: CanonicalValue::Long(i64::try_from(ordinal_offset).map_err(|_| {
                invalid(
                    "manager_batch_limit",
                    "Batch ordinal exceeds its scalar domain",
                )
            })?),
        }
    } else {
        draft.input("ordinal_input", ValueTypeTag::Long, false)?
    };
    let mut patterns = vec![Pattern::Let {
        binding: ordinal,
        expr: Expr::Operand(ordinal_input),
    }];
    if matches!(kind, WriteKind::Update | WriteKind::Attach) {
        let input = draft.input("target_iid", ValueTypeTag::String, false)?;
        patterns.extend([
            QueryPattern::Isa {
                binding: root,
                type_id: shape.type_id.clone(),
                include_subtypes: kind == WriteKind::Attach && shape.include_subtypes,
            }
            .into(),
            QueryPattern::Iid {
                binding: root,
                iid: input,
            }
            .into(),
        ]);
    }
    let target = if matches!(
        kind,
        WriteKind::PartialUpdate | WriteKind::Delete | WriteKind::DeleteReturning
    ) {
        Some(draft.input("target_iid", ValueTypeTag::String, true)?)
    } else {
        None
    };
    let identity_routes = rows
        .iter()
        .zip(&matrices)
        .map(|(row, (fields, _))| {
            if row.iid.is_some() {
                None
            } else {
                Some(
                    shape
                        .keys
                        .iter()
                        .filter(|key| fields.contains_key(*key))
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            }
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let route_input = if kind == WriteKind::PartialUpdate
        && complete
        && field_widths.keys().any(|field| !shape.keys.contains(field))
    {
        Some(draft.input("identity_route", ValueTypeTag::Long, false)?)
    } else {
        None
    };
    let mut attached = Vec::new();
    let mut keys = Vec::new();
    let mut identity_keys = Vec::new();
    let mut conditional_deleted = Vec::new();
    for (position, (field, (width, domain))) in field_widths.iter().enumerate() {
        let key = shape.keys.contains(field);
        for slot in 0..*width {
            let operand = draft.input(
                &format!("field_{position}_{slot}"),
                *domain,
                kind != WriteKind::MatchPut
                    && !(key && matches!(kind, WriteKind::Put | WriteKind::PartialPut)),
            )?;
            if matches!(
                kind,
                WriteKind::PartialUpdate | WriteKind::Delete | WriteKind::DeleteReturning
            ) && key
            {
                if slot == 0 {
                    identity_keys.push((field.clone(), operand));
                }
            } else if kind == WriteKind::MatchPut
                || (matches!(kind, WriteKind::Put | WriteKind::PartialPut) && key)
            {
                identity_keys.push((field.clone(), operand.clone()));
                keys.push((field.clone(), operand));
            } else if !matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
                if matches!(kind, WriteKind::PartialUpdate | WriteKind::PartialPut)
                    && complete
                    && slot == 0
                {
                    conditional_deleted.push((field.clone(), operand.clone()));
                }
                attached.push(AttachItem::Has {
                    optional: true,
                    owner: root,
                    field: field.clone(),
                    operand,
                });
            }
        }
    }
    let mut deleted_fields = Vec::new();
    if let Some(target) = &target {
        if kind == WriteKind::PartialUpdate && !complete {
            // Forced seeds contain one row, so their lookup needs no nullable
            // route disjunction. Keep the ordinal even when the target is absent.
            let mut lookup = vec![QueryPattern::Isa {
                binding: root,
                type_id: shape.type_id.clone(),
                include_subtypes: true,
            }];
            if rows[0].iid.is_some() {
                lookup.push(QueryPattern::Iid {
                    binding: root,
                    iid: target.clone(),
                });
            } else {
                for (field, input) in &identity_keys {
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
                            right: input.clone(),
                        },
                    ]);
                }
            }
            patterns.push(QueryPattern::Try { patterns: lookup }.into());
        } else {
            patterns.extend(
                identity_patterns(&mut draft, root, shape.type_id, target, &identity_keys)?
                    .into_iter()
                    .map(Pattern::Read),
            );
        }
        for (field, input) in &conditional_deleted {
            for (route_index, route) in identity_routes.iter().enumerate() {
                let owner = draft.bind("field_owner")?;
                let mut alias = vec![
                    QueryPattern::Isa {
                        binding: owner,
                        type_id: shape.type_id.clone(),
                        include_subtypes: true,
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: route_input.clone().expect("partial field route"),
                        right: QueryOperand::Literal {
                            value: CanonicalValue::Long(route_index as i64),
                        },
                    },
                    present(input),
                ];
                match route {
                    None => alias.push(QueryPattern::Iid {
                        binding: owner,
                        iid: target.clone(),
                    }),
                    Some(keys) => {
                        for key in keys {
                            let value = draft.bind("owner_key")?;
                            let (_, input) = identity_keys
                                .iter()
                                .find(|(field, _)| field == key)
                                .expect("identity key column");
                            alias.extend([
                                QueryPattern::Has {
                                    attribute: value,
                                    owner,
                                    attribute_id: key.attribute().clone(),
                                },
                                QueryPattern::Value {
                                    comparator: ValueComparator::Equal,
                                    left: QueryOperand::Binding { binding: value },
                                    right: input.clone(),
                                },
                            ]);
                        }
                    }
                }
                patterns.push(QueryPattern::Try { patterns: alias }.into());
                deleted_fields.push(DeleteItem::Has {
                    owner,
                    field: field.clone(),
                });
            }
        }
    }
    if kind == WriteKind::PartialPut {
        for (field, input) in conditional_deleted {
            let owner = draft.bind("field_owner")?;
            let mut alias = vec![
                QueryPattern::Isa {
                    binding: owner,
                    type_id: shape.type_id.clone(),
                    include_subtypes: true,
                },
                present(&input),
            ];
            for (key, value) in &identity_keys {
                let attribute = draft.bind("owner_key")?;
                alias.extend([
                    QueryPattern::Has {
                        attribute,
                        owner,
                        attribute_id: key.attribute().clone(),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: attribute },
                        right: value.clone(),
                    },
                ]);
            }
            patterns.push(QueryPattern::Try { patterns: alias }.into());
            deleted_fields.push(DeleteItem::Has { owner, field });
        }
    }
    // Key-changing updates can alter a referenced player's hydrated key.
    // Retain the source thing itself so its stable IID remains verifiable.
    let snapshot_roles = if kind == WriteKind::Update && !shape.references_see_prior_writes {
        player_widths
            .keys()
            .filter(|route| route.key.is_some() && route.candidates.contains(shape.type_id))
            .map(|route| route.role.clone())
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        std::collections::BTreeSet::new()
    };
    let mut reference_snapshots = Vec::new();
    for (position, (route, (width, domain))) in player_widths.iter().enumerate() {
        let snapshot_role = snapshot_roles.contains(&route.role).then(|| {
            shape
                .document_roles
                .iter()
                .position(|listed| listed.role == route.role)
                .expect("declared role")
        });
        for slot in 0..*width {
            let required = matrices
                .iter()
                .all(|(_, players)| players.get(route).is_some_and(|values| values.len() > slot));
            let input = draft.input(&format!("player_{position}_{slot}"), *domain, !required)?;
            if required {
                let player = draft.bind("player")?;
                if let Some(role) = snapshot_role {
                    reference_snapshots.push((role, player));
                }
                let branches = route
                    .candidates
                    .iter()
                    .map(|candidate| {
                        reference_patterns(
                            &mut draft,
                            player,
                            candidate,
                            route.key.as_ref(),
                            &input,
                        )
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                patterns.push(QueryPattern::Or { branches }.into());
                if matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
                    patterns.push(
                        QueryPattern::Links {
                            relation: root,
                            relation_id: shape.type_id.clone(),
                            players: vec![
                                type_bridge_contract::migration_assertion::AssertionRolePlayer::new(
                                    route.role.clone(),
                                    player,
                                ),
                            ],
                        }
                        .into(),
                    );
                } else {
                    attached.push(AttachItem::Links {
                        optional: false,
                        relation: root,
                        role: route.role.clone(),
                        player,
                    });
                }
            } else {
                let mut guards = vec![vec![QueryPattern::Not {
                    patterns: vec![QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: input.clone(),
                        right: input.clone(),
                    }],
                }]];
                for candidate in &route.candidates {
                    let witness = draft.bind("guard")?;
                    let mut guard = reference_patterns(
                        &mut draft,
                        witness,
                        candidate,
                        route.key.as_ref(),
                        &input,
                    )?;
                    if matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
                        guard.push(QueryPattern::Links {
                            relation: root,
                            relation_id: shape.type_id.clone(),
                            players: vec![
                                type_bridge_contract::migration_assertion::AssertionRolePlayer::new(
                                    route.role.clone(),
                                    witness,
                                ),
                            ],
                        });
                    }
                    guards.push(guard);
                    if matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
                        continue;
                    }
                    let player = draft.bind("optional_player")?;
                    if let Some(role) = snapshot_role {
                        reference_snapshots.push((role, player));
                    }
                    patterns.push(
                        QueryPattern::Try {
                            patterns: reference_patterns(
                                &mut draft,
                                player,
                                candidate,
                                route.key.as_ref(),
                                &input,
                            )?,
                        }
                        .into(),
                    );
                    attached.push(AttachItem::Links {
                        optional: true,
                        relation: root,
                        role: route.role.clone(),
                        player,
                    });
                }
                patterns.push(QueryPattern::Or { branches: guards }.into());
            }
        }
    }
    let patterns = if patterns.len() > type_bridge_contract::limits::MAX_BOOLEAN_TERMS {
        let mut scalar = Vec::new();
        let mut reads = Vec::new();
        for pattern in patterns {
            match pattern {
                Pattern::Read(pattern) => reads.push(pattern),
                other => scalar.push(other),
            }
        }
        scalar.extend(
            reads
                .chunks(type_bridge_contract::limits::MAX_BOOLEAN_TERMS)
                .map(|patterns| {
                    Pattern::from(QueryPattern::Or {
                        branches: vec![patterns.to_vec()],
                    })
                }),
        );
        scalar
    } else {
        patterns
    };
    let mut pipeline = vec![Stage::Match { patterns }];
    if !matches!(
        kind,
        WriteKind::Update
            | WriteKind::Attach
            | WriteKind::PartialUpdate
            | WriteKind::Delete
            | WriteKind::DeleteReturning
    ) {
        let players = if kind == WriteKind::MatchPut {
            let players = attached
                .iter()
                .filter_map(|item| match item {
                    AttachItem::Links { role, player, .. } => Some((role.clone(), *player)),
                    _ => None,
                })
                .collect();
            attached.retain(|item| !matches!(item, AttachItem::Links { .. }));
            players
        } else {
            Vec::new()
        };
        let things = vec![InsertThing {
            binding: root,
            type_id: shape.type_id.clone(),
            fields: keys,
            players,
        }];
        pipeline.push(
            if kind == WriteKind::MatchPut
                || (matches!(kind, WriteKind::Put | WriteKind::PartialPut)
                    && !shape.keys.is_empty())
            {
                Stage::Put {
                    include_subtypes: matches!(kind, WriteKind::PartialPut | WriteKind::MatchPut),
                    things,
                }
            } else {
                Stage::Insert { things }
            },
        );
    }
    if matches!(kind, WriteKind::Put | WriteKind::Update)
        || (matches!(kind, WriteKind::PartialUpdate | WriteKind::PartialPut) && !complete)
    {
        let items = shape
            .cleared_fields
            .iter()
            .filter(|field| kind != WriteKind::Put || !shape.keys.contains(field))
            .map(|field| DeleteItem::Has {
                owner: root,
                field: field.clone(),
            })
            .chain(shape.cleared_roles.iter().map(|role| DeleteItem::Players {
                relation: root,
                role: role.clone(),
            }))
            .collect::<Vec<_>>();
        if !items.is_empty() {
            pipeline.extend(
                items
                    .chunks(type_bridge_contract::limits::MAX_BOOLEAN_TERMS)
                    .map(|items| Stage::Delete {
                        items: items.to_vec(),
                    }),
            );
        }
    }
    if !deleted_fields.is_empty() {
        pipeline.extend(
            deleted_fields
                .chunks(type_bridge_contract::limits::MAX_BOOLEAN_TERMS)
                .map(|items| Stage::Delete {
                    items: items.to_vec(),
                }),
        );
    }
    if kind == WriteKind::Delete {
        pipeline.push(Stage::Delete {
            items: vec![DeleteItem::Thing { binding: root }],
        });
    }
    if !attached.is_empty() {
        pipeline.extend(
            attached
                .chunks(type_bridge_contract::limits::MAX_BOOLEAN_TERMS)
                .map(|items| Stage::Attach {
                    items: items.to_vec(),
                }),
        );
    }
    if kind == WriteKind::Delete || (kind == WriteKind::PartialUpdate && complete) {
        // Matched consumes the pipeline without fetching models for a void manager API.
    } else if kind == WriteKind::Attach && rows.len() > 1 {
        let events = draft.bind("events")?;
        pipeline.push(Stage::Reduce {
            assignments: vec![ReduceAssignment::new(events, Reducer::Count, None)],
            groups: vec![root, ordinal],
        });
    } else {
        let mut bindings = vec![root, ordinal];
        bindings.extend(reference_snapshots.iter().map(|(_, binding)| *binding));
        bindings.sort_unstable();
        bindings.dedup();
        pipeline.extend([Stage::Select { bindings }, Stage::Distinct]);
    }
    if complete
        && shape.return_documents
        && !matches!(
            kind,
            WriteKind::PartialUpdate | WriteKind::Delete | WriteKind::MatchPut
        )
    {
        let mut document = ThingDocument::new(root, shape.document_roles);
        document
            .scalars
            .push(type_bridge_contract::query_plan::v3::DocumentScalar {
                key: type_bridge_contract::migration_assertion::QueryVariable::new("ordinal")?,
                binding: ordinal,
            });
        for (role, binding) in reference_snapshots {
            document
                .scalars
                .push(type_bridge_contract::query_plan::v3::DocumentScalar {
                    key: type_bridge_contract::migration_assertion::QueryVariable::new(format!(
                        "reference_{role}_{}",
                        binding.get()
                    ))?,
                    binding,
                });
        }
        pipeline.push(Stage::Fetch(document));
    }
    if kind == WriteKind::DeleteReturning {
        pipeline.push(Stage::Delete {
            items: vec![DeleteItem::Thing { binding: root }],
        });
    }
    let mut inputs = Vec::with_capacity(rows.len());
    for (index, (row, (fields, players))) in rows.iter().zip(matrices).enumerate() {
        let mut cells = Vec::new();
        if !constant_ordinal {
            cells.push(Some(CanonicalValue::Long(
                i64::try_from(if kind == WriteKind::Attach {
                    ordinal_offset
                } else {
                    index + ordinal_offset
                })
                .map_err(|_| {
                    invalid(
                        "manager_batch_limit",
                        "Batch ordinal exceeds its scalar domain",
                    )
                })?,
            )));
        }
        if matches!(kind, WriteKind::Update | WriteKind::Attach) {
            cells.push(Some(iid(row.iid.ok_or_else(|| {
                invalid("manager_identity_missing", "An update requires an IID")
            })?)?));
        }
        if target.is_some() {
            cells.push(row.iid.map(iid).transpose()?);
        }
        if route_input.is_some() {
            let route = if row.iid.is_some() {
                None
            } else {
                Some(
                    shape
                        .keys
                        .iter()
                        .filter(|key| fields.contains_key(*key))
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            };
            cells.push(Some(CanonicalValue::Long(
                identity_routes
                    .iter()
                    .position(|candidate| candidate == &route)
                    .expect("input identity route") as i64,
            )));
        }
        for (field, (width, _)) in &field_widths {
            let values = fields.get(field).map_or(&[][..], Vec::as_slice);
            cells.extend((0..*width).map(|slot| values.get(slot).cloned()));
        }
        for (route, (width, _)) in &player_widths {
            let values = players.get(route).map_or(&[][..], Vec::as_slice);
            cells.extend((0..*width).map(|slot| values.get(slot).cloned()));
        }
        inputs.push(InputRow::new(cells));
    }
    let operation = if pipeline.iter().any(Stage::is_write) {
        Operation::Write {
            returns: if kind == WriteKind::Delete || (kind == WriteKind::PartialUpdate && complete)
            {
                WriteReturns::Matched
            } else {
                WriteReturns::Rows
            },
        }
    } else if kind == WriteKind::PartialUpdate {
        Operation::Count
    } else {
        Operation::Rows
    };
    let chunks = super::input::chunks(
        inputs,
        0,
        super::input::canonical_upper_bound,
        |row| {
            usize::try_from(crate::projected_batch_executor::given_rows_bytes(row)?).map_err(|_| {
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure()
            })
        },
        |_, _| false,
    )
    .map_err(|error| invalid(error.code().as_str(), error.message().as_str()))?;
    let mut chunks = chunks.into_iter();
    let first = chunks.next().expect("nonempty write inputs");
    let template = draft.finish_rows(
        authority,
        pipeline,
        operation,
        QueryOutput::Rows {
            columns: if complete
                && (matches!(kind, WriteKind::PartialUpdate | WriteKind::Delete)
                    || (shape.return_documents && kind != WriteKind::MatchPut))
            {
                Vec::new()
            } else {
                vec![ordinal, root]
            },
        },
        Some(first),
    )?;
    let mut plans = Vec::with_capacity(chunks.len() + 1);
    for rows in chunks {
        plans.push(BuiltPlan {
            plan: template
                .local()
                .with_given_rows(rows, authority.declared())?,
            verified: template.verified,
        });
    }
    plans.insert(0, template);
    Ok(plans)
}

pub(crate) fn write_many(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<WriteProgram, Diagnostic> {
    let sequential_visibility = matches!(
        kind,
        WriteKind::PartialPut | WriteKind::MatchPut | WriteKind::PartialUpdate
    ) || (shape.references_see_prior_writes
        && matches!(kind, WriteKind::Insert | WriteKind::Put));
    if sequential_visibility && sequence::requires_sequence(authority, kind, &shape, rows)? {
        return sequence::build(authority, kind, shape, rows);
    }
    let program = match build_many(authority, kind, shape.clone(), rows, true, 0) {
        Ok(plans) => Ok(WriteProgram::compact(plans)),
        Err(error) if kind == WriteKind::MatchPut && program::forces_pieces(&error) => {
            sequence::build(authority, kind, shape.clone(), rows)
        }
        Err(error) if program::forces_pieces(&error) => {
            program::split(authority, kind, shape.clone(), rows)
        }
        Err(error) => Err(error),
    }?;
    // Other model types cannot be changed by this exact-type batch. Mutable
    // same-model references need invocation-start identities across cuts.
    let mutable_references = !shape.references_see_prior_writes
        && rows.iter().any(|row| {
            row.players.iter().any(|(_, reference)| {
                reference.candidates.contains(shape.type_id)
                    && (rows.len() > 1
                        || (kind == WriteKind::Update
                            && matches!(reference.identity, ReferenceIdentity::Key { .. })))
            })
        });
    if mutable_references && program.statement_count() > 1 {
        drop(program);
        program::resolve_references(authority, kind, shape, rows)
    } else {
        Ok(program)
    }
}
