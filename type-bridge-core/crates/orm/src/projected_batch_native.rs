//! Native plan execution retains the common batch lease, event matrix and consumers.

use super::*;
use crate::plan_builders::{
    BuiltPlan,
    batch_native::{self as build, BatchDraft},
};
use crate::query_plan_exec::{self, remote_value};
use crate::session::backend::QueryResult;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::v3::{AttachItem, DeleteItem, InsertThing, Stage};
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern, ReduceAssignment, Reducer};
use type_bridge_contract::query_remote::RemoteValue;

fn diagnostic(error: Diagnostic) -> SdkExecutionDiagnostic {
    lower_orm_error(
        &crate::plan_builders::manager::diagnostic(error),
        SdkProviderOperation::Write,
    )
}

impl BatchPlan<'_> {
    fn declaring_field(
        &self,
        field: &OwnsFactId,
    ) -> std::result::Result<&OwnsFactId, SdkExecutionDiagnostic> {
        self.model
            .query_tokens()
            .fields()
            .get(field)
            .map(|token| token.declaring_id())
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }

    fn domain(
        &self,
        field: &OwnsFactId,
    ) -> std::result::Result<ValueTypeTag, SdkExecutionDiagnostic> {
        self.installed
            .projection()
            .models()
            .get(&attribute_type(field))
            .and_then(|model| model.declaration().value_type())
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }

    pub(super) fn native_prerequisite(
        &self,
        rows: GivenRowsSpec,
    ) -> std::result::Result<BuiltPlan, SdkExecutionDiagnostic> {
        let put_keys = self.model.reference_read().key_fields();
        let mut columns = vec![(ValueTypeTag::Long, false); 4];
        columns.push((ValueTypeTag::String, true));
        for field in put_keys {
            columns.push((self.domain(field)?, true));
        }
        for route in &self.reference_routes {
            if let ReferenceRouteKey::Key(_, _, field) = &route.key {
                columns.push((self.domain(field)?, true));
            }
        }
        let mut draft = BatchDraft::new(rows, &columns).map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let kind = draft
            .bind_input(0, "kind", &mut patterns)
            .map_err(diagnostic)?;
        let ordinal = draft
            .bind_input(1, "ordinal", &mut patterns)
            .map_err(diagnostic)?;
        let reference = draft
            .bind_input(2, "reference", &mut patterns)
            .map_err(diagnostic)?;
        let thing = draft.draft.bind("thing").map_err(diagnostic)?;
        let mut branches = Vec::new();
        if self.operation == ProjectedBatchOperation::Put {
            let mut branch = vec![
                build::equal(draft.inputs[0].clone(), build::literal(0)),
                build::isa(thing, self.model.id()),
            ];
            for (index, field) in put_keys.iter().enumerate() {
                let value = draft.draft.bind("key").map_err(diagnostic)?;
                branch.extend([
                    QueryPattern::Has {
                        owner: thing,
                        attribute: value,
                        attribute_id: field.attribute().clone(),
                    },
                    build::equal(
                        QueryOperand::Binding { binding: value },
                        draft.inputs[5 + index].clone(),
                    ),
                ]);
            }
            branches.push(branch);
        }
        let mut key_column = 5 + put_keys.len();
        for (index, route) in self.reference_routes.iter().enumerate() {
            let operand = if route.key_variable.is_some() {
                let operand = draft.inputs[key_column].clone();
                key_column += 1;
                Some(operand)
            } else {
                None
            };
            for candidate in &route.candidate_types {
                let mut branch = vec![
                    build::equal(draft.inputs[0].clone(), build::literal(1)),
                    build::equal(draft.inputs[3].clone(), build::literal(index as i64)),
                    build::isa(thing, candidate),
                ];
                match &route.key {
                    ReferenceRouteKey::Iid(_, _) => branch.push(QueryPattern::Iid {
                        binding: thing,
                        iid: draft.inputs[4].clone(),
                    }),
                    ReferenceRouteKey::Key(_, _, field) => {
                        let value = draft.draft.bind("key").map_err(diagnostic)?;
                        branch.extend([
                            QueryPattern::Has {
                                owner: thing,
                                attribute: value,
                                attribute_id: field.attribute().clone(),
                            },
                            build::equal(
                                QueryOperand::Binding { binding: value },
                                operand.clone().expect("key route operand"),
                            ),
                        ]);
                    }
                }
                branches.push(branch);
            }
        }
        patterns.push(QueryPattern::Or { branches }.into());
        draft
            .finish(
                self.installed
                    .plan_authority()
                    .map_err(diagnostic)?
                    .as_ref(),
                vec![Stage::Match { patterns }],
                false,
                vec![kind, ordinal, reference, thing],
            )
            .map_err(diagnostic)
    }

    fn native_mutation(
        &self,
        rows: GivenRowsSpec,
        first: Option<&RoleRouteKey>,
        operation: ProjectedBatchOperation,
    ) -> std::result::Result<BuiltPlan, SdkExecutionDiagnostic> {
        let mut columns = vec![(ValueTypeTag::Long, false)];
        match operation {
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => {
                columns.push((ValueTypeTag::String, false))
            }
            ProjectedBatchOperation::Put => {
                for key in self.model.reference_read().key_fields() {
                    columns.push((self.domain(key)?, false));
                }
            }
            _ => {}
        }
        if first.is_some() {
            columns.push((ValueTypeTag::String, false));
        }
        let mut draft = BatchDraft::new(rows, &columns).map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let ordinal = draft
            .bind_input(0, "ordinal", &mut patterns)
            .map_err(diagnostic)?;
        let thing = draft.draft.bind("thing").map_err(diagnostic)?;
        let mut players = Vec::new();
        if let Some(first) = first {
            let player = draft.draft.bind("first").map_err(diagnostic)?;
            patterns.push(build::isa(player, &first.player_type).into());
            patterns.push(
                QueryPattern::Iid {
                    binding: player,
                    iid: draft.inputs.last().expect("first player input").clone(),
                }
                .into(),
            );
            players.push((first.role.clone(), player));
        }
        if matches!(
            operation,
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete
        ) {
            patterns.push(build::isa(thing, self.model.id()).into());
            patterns.push(
                QueryPattern::Iid {
                    binding: thing,
                    iid: draft.inputs[1].clone(),
                }
                .into(),
            );
        }
        let mut pipeline = vec![Stage::Match { patterns }];
        if matches!(
            operation,
            ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put
        ) {
            let fields = if operation == ProjectedBatchOperation::Put {
                self.model
                    .reference_read()
                    .key_fields()
                    .iter()
                    .enumerate()
                    .map(|(index, key)| {
                        Ok((
                            self.declaring_field(key)?.clone(),
                            draft.inputs[index + 1].clone(),
                        ))
                    })
                    .collect::<std::result::Result<Vec<_>, SdkExecutionDiagnostic>>()?
            } else {
                Vec::new()
            };
            let things = vec![InsertThing {
                binding: thing,
                type_id: self.model.id().clone(),
                fields,
                players,
            }];
            pipeline.push(if operation == ProjectedBatchOperation::Put {
                Stage::Put {
                    things,
                    include_subtypes: false,
                }
            } else {
                Stage::Insert { things }
            });
        }
        if matches!(
            operation,
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Put
        ) {
            for field in self.model.complete_read().fields() {
                if operation == ProjectedBatchOperation::Put
                    && self
                        .model
                        .reference_read()
                        .key_fields()
                        .contains(field.token())
                {
                    continue;
                }
                pipeline.push(Stage::Delete {
                    items: vec![DeleteItem::Has {
                        owner: thing,
                        field: self.declaring_field(field.token())?.clone(),
                    }],
                });
                pipeline.push(Stage::Select {
                    bindings: vec![ordinal, thing],
                });
                pipeline.push(Stage::Distinct);
            }
            for role in self.model.complete_read().roles().keys() {
                pipeline.push(Stage::Delete {
                    items: vec![DeleteItem::Players {
                        relation: thing,
                        role: role.clone(),
                    }],
                });
                pipeline.push(Stage::Select {
                    bindings: vec![ordinal, thing],
                });
                pipeline.push(Stage::Distinct);
            }
        }
        if operation == ProjectedBatchOperation::Delete {
            pipeline.push(Stage::Delete {
                items: vec![DeleteItem::Thing { binding: thing }],
            });
            return draft
                .finish(
                    self.installed
                        .plan_authority()
                        .map_err(diagnostic)?
                        .as_ref(),
                    pipeline,
                    true,
                    vec![ordinal],
                )
                .map_err(diagnostic);
        }
        draft
            .finish(
                self.installed
                    .plan_authority()
                    .map_err(diagnostic)?
                    .as_ref(),
                pipeline,
                true,
                vec![ordinal, thing],
            )
            .map_err(diagnostic)
    }

    fn native_attach(
        &self,
        rows: GivenRowsSpec,
        snapshot: bool,
    ) -> std::result::Result<BuiltPlan, SdkExecutionDiagnostic> {
        let mut columns = vec![(ValueTypeTag::Long, false), (ValueTypeTag::String, false)];
        if !snapshot {
            for field in &self.field_routes {
                columns.push((self.domain(&field.field)?, true));
            }
            columns.extend(
                self.role_routes
                    .iter()
                    .map(|_| (ValueTypeTag::String, true)),
            );
        }
        let mut draft = BatchDraft::new(rows, &columns).map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let ordinal = draft
            .bind_input(0, "ordinal", &mut patterns)
            .map_err(diagnostic)?;
        let thing = draft.draft.bind("thing").map_err(diagnostic)?;
        patterns.extend([
            build::isa(thing, self.model.id()).into(),
            QueryPattern::Iid {
                binding: thing,
                iid: draft.inputs[1].clone(),
            }
            .into(),
        ]);
        let mut items = Vec::new();
        if !snapshot {
            for (index, field) in self.field_routes.iter().enumerate() {
                items.push(AttachItem::Has {
                    optional: true,
                    owner: thing,
                    field: self.declaring_field(&field.field)?.clone(),
                    operand: draft.inputs[index + 2].clone(),
                });
            }
            for (index, role) in self.role_routes.iter().enumerate() {
                let player = draft.draft.bind("player").map_err(diagnostic)?;
                patterns.push(
                    QueryPattern::Try {
                        patterns: vec![
                            build::isa(player, &role.key.player_type),
                            QueryPattern::Iid {
                                binding: player,
                                iid: draft.inputs[index + 2 + self.field_routes.len()].clone(),
                            },
                        ],
                    }
                    .into(),
                );
                items.push(AttachItem::Links {
                    optional: true,
                    relation: thing,
                    role: role.key.role.clone(),
                    player,
                });
            }
        }
        let mut pipeline = vec![Stage::Match { patterns }];
        if !items.is_empty() {
            pipeline.push(Stage::Attach { items });
        }
        pipeline.push(Stage::Select {
            bindings: vec![ordinal, thing],
        });
        pipeline.push(Stage::Distinct);
        if self.model.id().kind() == TypeKind::Relation && !snapshot {
            let count = draft.draft.bind("events").map_err(diagnostic)?;
            pipeline.push(Stage::Reduce {
                assignments: vec![ReduceAssignment::new(count, Reducer::Count, None)],
                groups: vec![ordinal, thing],
            });
        }
        pipeline.push(Stage::Fetch(
            build::document(
                thing,
                ordinal,
                self.model.complete_read().roles().keys().cloned(),
            )
            .map_err(diagnostic)?,
        ));
        draft
            .finish(
                self.installed
                    .plan_authority()
                    .map_err(diagnostic)?
                    .as_ref(),
                pipeline,
                !snapshot,
                vec![],
            )
            .map_err(diagnostic)
    }

    fn native_mutation_groups(
        &self,
        resolved: &ResolvedPrerequisites,
    ) -> std::result::Result<
        Vec<(ProjectedBatchOperation, Option<RoleRouteKey>, GivenRowsSpec)>,
        SdkExecutionDiagnostic,
    > {
        if self.model.id().kind() != TypeKind::Relation
            || !matches!(
                self.operation,
                ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put
            )
        {
            return Ok(vec![(self.operation, None, self.mutation_rows(resolved)?)]);
        }
        let mut firsts = BTreeMap::new();
        for (index, reference) in self.role_references.iter().enumerate() {
            firsts
                .entry(reference.owner_ordinal)
                .or_insert((reference, resolved.reference(index)?));
        }
        let mut groups: BTreeMap<Option<RoleRouteKey>, Vec<Vec<GivenValue>>> = BTreeMap::new();
        for (ordinal, _) in &self.rows {
            if self.operation == ProjectedBatchOperation::Put
                && let Some(iid) = &resolved.put_hits[*ordinal]
            {
                groups.entry(None).or_default().push(vec![
                    ordinal_value(*ordinal)?,
                    GivenValue::String(iid.clone()),
                ]);
                continue;
            }
            let (reference, resolved) = firsts
                .get(ordinal)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let key = RoleRouteKey {
                role: reference.role.clone(),
                player_type: resolved.concrete_type.clone(),
            };
            groups.entry(Some(key)).or_default().push(vec![
                ordinal_value(*ordinal)?,
                GivenValue::String(resolved.iid.clone()),
            ]);
        }
        Ok(groups
            .into_iter()
            .map(|(key, rows)| {
                let operation = if key.is_some() {
                    ProjectedBatchOperation::Insert
                } else {
                    ProjectedBatchOperation::Update
                };
                let variables = vec![
                    "ordinal".into(),
                    if key.is_some() {
                        "first-iid".into()
                    } else {
                        "target-iid".into()
                    },
                ];
                (operation, key, GivenRowsSpec { variables, rows })
            })
            .collect())
    }
}

impl ProjectedBatchExecutor<'_> {
    pub(super) async fn run_native(
        &self,
        lease: &mut TransactionContextMutationLease,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        plan: &mut BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        if plan.operation == ProjectedBatchOperation::Delete && plan.snapshot_source.is_some() {
            let registry = plan.installed.manager_registry().map_err(|error| {
                at_type(
                    lower_orm_error(&error, SdkProviderOperation::Read),
                    plan.model.id(),
                )
            })?;
            let plans = crate::plan_builders::manager::ManagerPlans::new(registry);
            let iids = plan
                .rows
                .iter()
                .map(|(_, row)| row_iid(row).expect("normalized delete IID").to_owned())
                .collect::<Vec<_>>();
            let input_bytes = prepared.batch().resource_measure().bytes();
            let mut deletion_policy = prepared.limits();
            if input_bytes > deletion_policy.bytes {
                return Err(resource_limit(
                    "projected_batch_transport_byte_limit",
                    "Deletion input exceeds the byte ceiling",
                    "bytes",
                    input_bytes,
                    deletion_policy.bytes,
                ));
            }
            deletion_policy.bytes -= input_bytes;
            let rows = crate::_manager::returning::delete_on_lease(
                lease,
                &plans,
                plan.model.id().label().as_str(),
                &iids,
                true,
                deletion_policy,
                prepared.cancellation(),
                prepared.deadline(),
                |registry, things, _, remaining| {
                    let rows = things
                        .iter()
                        .map(|thing| match plan.model.id().kind() {
                            TypeKind::Entity => {
                                crate::_manager::dynamic::plan_entity_row(registry, thing)
                                    .map(DeletedRow::Entity)
                            }
                            TypeKind::Relation => {
                                crate::_manager::dynamic::plan_relation_row(registry, thing)
                                    .map(DeletedRow::Relation)
                            }
                            _ => Err(crate::plan_builders::manager::invalid(
                                "manager_result_shape",
                                "Deletion requires an entity or relation",
                            )),
                        })
                        .collect::<crate::Result<Vec<_>>>()?;
                    Ok((rows, remaining))
                },
            )
            .await
            .map_err(|error| {
                at_type(
                    lower_orm_error(&error, SdkProviderOperation::Write),
                    plan.model.id(),
                )
            })?;
            let (rows, remaining) = rows;
            let mut things = Vec::with_capacity(rows.len());
            for row in rows {
                let mut thing = match row {
                    DeletedRow::Entity(row) => {
                        let iid = row
                            .iid
                            .clone()
                            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                        self.project_entity(row, plan.model.id(), &iid, identity)?
                    }
                    DeletedRow::Relation(row) => {
                        let iid = row
                            .iid
                            .clone()
                            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                        self.project_relation(row, plan.model.id(), &iid, identity)?
                    }
                };
                thing.mark_deleted()?;
                things.push(thing);
            }
            let mut ledger = ExecutionLedger::new(remaining, 0);
            ledger.charge_projected(&things)?;
            prepared.check_control()?;
            return Ok(ProjectedBatchResult::Things(things));
        }
        let authority = plan.installed.plan_authority().map_err(diagnostic)?;
        let mut ledger = ExecutionLedger::new(
            prepared.limits(),
            prepared.batch().resource_measure().bytes(),
        );
        let mut preflight = ExecutionLedger::new(
            prepared.limits(),
            prepared.batch().resource_measure().bytes(),
        );
        preflight.charge_preflight(plan.preflight_bytes, plan.statement_count)?;
        ledger.charge_preflight(0, plan.statement_count)?;
        let mut prerequisite_statements = 0_usize;
        let mut resolved = ResolvedPrerequisites::new(plan)?;
        if let Some(statement) = plan.prerequisite.take() {
            let mut consumer = PrerequisiteConsumer::new(plan, &mut resolved);
            for chunk in input_chunks(statement.rows, true)? {
                prerequisite_statements += 1;
                ledger.limits = prepared
                    .limits()
                    .for_adapter_statements(prerequisite_statements + 2);
                ledger.charge_preflight(0, (prerequisite_statements + 2) as u32)?;
                let native = plan.native_prerequisite(chunk)?;
                dispatch(
                    lease,
                    &native,
                    Reply::Prerequisite,
                    prepared,
                    &mut consumer,
                    &mut ledger,
                    true,
                    &authority,
                )
                .await
                .map_err(|later| consumer.take_deferred().unwrap_or(later))?;
            }
            consumer.finish()?;
        }
        resolved.finish(plan)?;
        if plan.operation != ProjectedBatchOperation::Delete {
            ledger.require_remaining_items(
                (plan.row_count as u64)
                    .saturating_mul(2)
                    .saturating_add(plan.role_references.len() as u64),
            )?;
        }
        let groups = plan.native_mutation_groups(&resolved)?;
        // Build every mutation before the first write, so invalid groups cannot partially execute.
        let mut mutations = Vec::new();
        for (operation, first, rows) in groups {
            for chunk in input_chunks(rows, false)? {
                mutations.push(plan.native_mutation(chunk, first.as_ref(), operation)?);
            }
        }
        let mut pending_attach = if plan.operation == ProjectedBatchOperation::Delete {
            None
        } else {
            Some(input_chunks_reserved(
                plan.attach_rows(&resolved, None)?,
                false,
                MAX_PROVIDER_IID_BYTES as usize,
            )?)
        };
        let statement_count =
            prerequisite_statements + mutations.len() + pending_attach.as_ref().map_or(0, Vec::len);
        ledger.limits = prepared.limits().for_adapter_statements(statement_count);
        ledger.charge_preflight(
            0,
            u32::try_from(statement_count).map_err(|_| complexity_overflow())?,
        )?;
        let mut identities = OrdinalIdentityConsumer::new(plan.row_count, plan.operation)?;
        // A missing exact delete is idempotent; only present targets produce native rows.
        for native in mutations {
            dispatch(
                lease,
                &native,
                Reply::Mutation,
                prepared,
                &mut identities,
                &mut ledger,
                false,
                &authority,
            )
            .await
            .map_err(|later| identities.take_deferred().unwrap_or(later))?;
        }
        if plan.operation == ProjectedBatchOperation::Delete {
            for slot in &mut identities.slots {
                if slot.is_none() {
                    *slot = Some(String::new());
                }
            }
        }
        identities.finish(plan, &resolved)?;
        if plan.operation == ProjectedBatchOperation::Delete {
            prepared.check_control()?;
            return Ok(ProjectedBatchResult::Deleted);
        }
        let mut consumer = HydrationConsumer::new(self, plan, identity)?;
        identities.move_iids_into(&mut consumer.targets)?;
        for mut rows in pending_attach.take().expect("non-delete attach rows") {
            fill_target_iids(&mut rows, &consumer.targets)?;
            let native = plan.native_attach(rows, false)?;
            dispatch(
                lease,
                &native,
                Reply::Hydration,
                prepared,
                &mut consumer,
                &mut ledger,
                false,
                &authority,
            )
            .await
            .map_err(|later| consumer.take_deferred().unwrap_or(later))?;
        }
        let things = consumer.finish()?;
        ledger.charge_projected(&things)?;
        prepared.check_control()?;
        Ok(ProjectedBatchResult::Things(things))
    }
}

fn input_row_upper_bound(row: &[GivenValue]) -> usize {
    row.iter().fold(2_usize, |total, value| {
        let text = match value {
            GivenValue::String(value)
            | GivenValue::Date(value)
            | GivenValue::Datetime(value)
            | GivenValue::DatetimeTz(value)
            | GivenValue::Decimal(value) => value.len(),
            GivenValue::DatetimeTzExact {
                local, named_zone, ..
            } => local
                .len()
                .saturating_add(named_zone.as_ref().map_or(0, String::len)),
            _ => 0,
        };
        total
            .saturating_add(512)
            .saturating_add(text.saturating_mul(6))
    })
}

#[cfg(test)]
fn input_upper_bound(rows: &GivenRowsSpec) -> usize {
    rows.rows.iter().fold(2_usize, |total, row| {
        total.saturating_add(input_row_upper_bound(row))
    })
}

fn input_chunks(
    rows: GivenRowsSpec,
    lookup: bool,
) -> std::result::Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    input_chunks_reserved(rows, lookup, 0)
}

fn input_chunks_reserved(
    rows: GivenRowsSpec,
    lookup: bool,
    reserved_bytes_per_row: usize,
) -> std::result::Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    let GivenRowsSpec { variables, rows } = rows;
    crate::plan_builders::input::chunks(
        rows,
        reserved_bytes_per_row,
        |row| input_row_upper_bound(row),
        |row| {
            let canonical = type_bridge_contract::query_plan::InputRow::new(
                row.iter()
                    .cloned()
                    .map(build::canonical)
                    .collect::<std::result::Result<_, _>>()
                    .map_err(diagnostic)?,
            );
            usize::try_from(given_rows_bytes(&canonical)?).map_err(|_| complexity_overflow())
        },
        |previous, row| !lookup && previous.first() == row.first(),
    )
    .map(|chunks| {
        chunks
            .into_iter()
            .map(|rows| GivenRowsSpec {
                variables: variables.clone(),
                rows,
            })
            .collect()
    })
}

enum DeletedRow {
    Entity(DynamicEntityRow),
    Relation(DynamicRelationRow),
}

#[derive(Clone, Copy)]
enum Reply {
    Prerequisite,
    Mutation,
    Hydration,
}

struct Consumer<'a> {
    inner: &'a mut dyn AnswerConsumer,
    plan: &'a type_bridge_contract::query_plan::v3::QueryPlanV3,
    reply: Reply,
    deferred: Option<SdkExecutionDiagnostic>,
}

fn evidence(error: Diagnostic) -> OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::ResultDecode,
        error.code().as_str(),
        "Native batch returned invalid typed evidence",
    )
    .into()
}
impl Consumer<'_> {
    fn decode(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        let document = match self.reply {
            Reply::Hydration => {
                let AnswerItem::Document(document) = item else {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResultDecode,
                        "query_plan_batch_answer",
                        "Native batch fetch returned rows",
                    )
                    .into());
                };
                let fields =
                    query_plan_exec::remote_document(&document, self.plan).map_err(evidence)?;
                let mut document = query_plan_exec::document_json(&fields).map_err(evidence)?;
                let fetched = self
                    .plan
                    .pipeline()
                    .iter()
                    .find_map(|stage| {
                        if let Stage::Fetch(document) = stage {
                            Some(document)
                        } else {
                            None
                        }
                    })
                    .expect("fetch shape");
                let mut players = Vec::new();
                for (index, role) in fetched.roles.iter().enumerate() {
                    let list = document
                        .as_object_mut()
                        .expect("validated object")
                        .remove(&format!("_role_{index}_players"))
                        .expect("validated role list");
                    let Value::Array(list) = list else {
                        unreachable!("validated list")
                    };
                    for player in list {
                        players.push(serde_json::json!({"role": format!("{}:{}", role.role.declaring_relation(), role.role.label()), "iid": player["_iid"], "type_name": player["_type"], "attributes": player["attributes"]}));
                    }
                }
                if !fetched.roles.is_empty() {
                    document["role_players"] = players.into();
                }
                document
            }
            Reply::Prerequisite | Reply::Mutation => {
                let AnswerItem::Row(row) = item else {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResultDecode,
                        "query_plan_batch_answer",
                        "Native batch scalar output returned documents",
                    )
                    .into());
                };
                let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                    self.plan.output()
                else {
                    unreachable!("row output")
                };
                let names = match self.reply {
                    Reply::Prerequisite => &["kind", "ordinal", "reference_ordinal", "thing"][..],
                    _ => &["ordinal", "iid"][..],
                };
                let mut document = serde_json::Map::new();
                for (column, key) in columns.iter().zip(names) {
                    let name = self.plan.bindings()[column.get() as usize]
                        .variable()
                        .as_str();
                    let value =
                        remote_value(&row, name, self.plan.limits().codec).map_err(evidence)?;
                    match value {
                        RemoteValue::Thing { iid, type_id } => {
                            document.insert("iid".into(), iid.into());
                            if matches!(self.reply, Reply::Prerequisite) {
                                document.insert("type".into(), type_id.label().as_str().into());
                            }
                        }
                        RemoteValue::Absent
                            if matches!(self.reply, Reply::Mutation) && *key == "iid" => {}
                        RemoteValue::Value { value } | RemoteValue::Attribute { value, .. } => {
                            document.insert(
                                (*key).into(),
                                query_plan_exec::scalar_json(&value).map_err(evidence)?,
                            );
                        }
                        _ => {
                            return Err(crate::match_request::MatchError::new(
                                crate::match_request::MatchErrorCategory::ResultDecode,
                                "query_plan_batch_answer",
                                "Native batch scalar output has an invalid concept",
                            )
                            .into());
                        }
                    }
                }
                document.into()
            }
        };
        self.inner.accept(AnswerItem::Document(document))
    }
}

impl AnswerConsumer for Consumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        if self.deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        match self.decode(item) {
            Ok(control) => Ok(control),
            Err(error) => {
                self.deferred = Some(lower_orm_error(&error, SdkProviderOperation::Read));
                Ok(AnswerControl::Continue)
            }
        }
    }
}

pub(super) fn accept_prerequisite_result(
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    answer: QueryResult,
    inner: &mut dyn AnswerConsumer,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let QueryResult::Rows(rows) = answer else {
        return Err(SdkExecutionDiagnostic::internal_failure());
    };
    let mut consumer = Consumer {
        inner,
        plan,
        reply: Reply::Prerequisite,
        deferred: None,
    };
    for row in rows {
        consumer
            .accept(AnswerItem::Row(row))
            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Read))?;
    }
    consumer.deferred.map_or(Ok(()), Err)
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    lease: &mut TransactionContextMutationLease,
    plan: &BuiltPlan,
    reply: Reply,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    ledger: &mut ExecutionLedger,
    lookup: bool,
    authority: &crate::plan_builders::authority::PlanAuthority,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let mut query = lease
        .prepare_plan(plan.local())
        .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
    let rows = query.rows.take().expect("native batch rows");
    ledger.charge_bytes(
        checked_add_u64(source_bytes(&query.typeql)?, given_rows_bytes(&rows)?)?,
        "projected_batch_transport_byte_limit",
    )?;
    let chunks = if lookup {
        split_lookups(&query.typeql, rows, SplitLimits::CANONICAL)?
    } else {
        split_stage(&query.typeql, rows, SplitLimits::CANONICAL)?
    };
    if chunks.len() == 1 {
        query.rows = chunks.into_iter().next();
        return dispatch_query(
            lease,
            plan.local(),
            query,
            reply,
            prepared,
            consumer,
            ledger,
        )
        .await;
    }
    for chunk in chunks {
        let rows = chunk
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(build::canonical)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map(type_bridge_contract::query_plan::InputRow::new)
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(diagnostic)?;
        let local = plan
            .local()
            .with_given_rows(rows, authority.declared())
            .map_err(diagnostic)?;
        let query = lease
            .prepare_plan(&local)
            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
        dispatch_query(lease, &local, query, reply, prepared, consumer, ledger).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_query(
    lease: &mut TransactionContextMutationLease,
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    query: crate::session::staged::StagedQuery,
    reply: Reply,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    ledger: &mut ExecutionLedger,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    prepared.check_control()?;
    let limits = AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: ledger.remaining_items(),
            max_bytes: ledger.remaining_bytes(),
            deadline: Some(prepared.deadline().instant()),
            cancellation: prepared.cancellation().clone(),
        },
        max_collection_members: prepared.limits().collection_members,
        ..Default::default()
    };
    let mut consumer = Consumer {
        inner: consumer,
        plan,
        reply,
        deferred: None,
    };
    let mut observed = LocallyBoundedConsumer::new(&mut consumer, limits.answer.clone());
    let future = lease.dispatch_plan_bounded(query, limits, &mut observed);
    let dispatched = await_controlled(future, prepared.deadline(), prepared.cancellation()).await;
    let measured = observed.stats();
    drop(observed);
    if let Some(first) = consumer.deferred {
        return Err(first);
    }
    let mut stats = dispatched.map_err(|error| {
        controlled_orm(
            error,
            if plan.writes() {
                SdkProviderOperation::Write
            } else {
                SdkProviderOperation::Read
            },
            prepared.batch().model(),
        )
    })?;
    stats.processed_items = stats.processed_items.max(measured.processed_items);
    stats.response_bytes = stats.response_bytes.max(measured.response_bytes);
    stats.stopped_early |= measured.stopped_early;
    ledger.record_reply(stats)?;
    Ok(())
}

#[cfg(test)]
mod native_input_tests {
    use super::*;
    use type_bridge_contract::limits::{MAX_CANONICAL_STRING_BYTES, MAX_INPUT_BYTES};

    fn rows(values: Vec<(i64, String)>) -> GivenRowsSpec {
        GivenRowsSpec {
            variables: vec!["ordinal".into(), "field".into()],
            rows: values
                .into_iter()
                .map(|(ordinal, value)| {
                    vec![GivenValue::Integer(ordinal), GivenValue::String(value)]
                })
                .collect(),
        }
    }
    fn exact(rows: &GivenRowsSpec) -> usize {
        let canonical = rows
            .rows
            .iter()
            .map(|row| {
                type_bridge_contract::query_plan::InputRow::new(
                    row.iter()
                        .cloned()
                        .map(build::canonical)
                        .collect::<std::result::Result<_, _>>()
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        given_rows_bytes(&canonical).unwrap() as usize
    }

    #[test]
    fn canonical_input_boundary_uses_a_conservative_fast_guard_and_exact_slow_path() {
        let just_under = rows(
            (0..4)
                .map(|ordinal| (ordinal, "x".repeat(MAX_CANONICAL_STRING_BYTES - 128)))
                .collect(),
        );
        let size = exact(&just_under);
        assert!(
            size < MAX_INPUT_BYTES && size > MAX_INPUT_BYTES - 1024,
            "{size}"
        );
        assert!(input_upper_bound(&just_under) > MAX_INPUT_BYTES);
        assert_eq!(input_chunks(just_under, false).unwrap().len(), 1);

        let escaped = rows(vec![(0, "\0".repeat((MAX_INPUT_BYTES - 1030) / 6))]);
        let size = exact(&escaped);
        assert!(
            size > MAX_INPUT_BYTES - 2048 && size < MAX_INPUT_BYTES,
            "{size}"
        );
        assert!(input_upper_bound(&escaped) >= size);
        assert!(input_upper_bound(&escaped) <= MAX_INPUT_BYTES);
        assert_eq!(input_chunks(escaped, false).unwrap().len(), 1);
    }

    #[test]
    fn attach_input_reserves_target_iids_on_the_exact_size_path() {
        let empty = rows(
            (0..4)
                .map(|ordinal| (ordinal, "x".repeat(MAX_CANONICAL_STRING_BYTES - 128)))
                .collect(),
        );
        let mut attach = GivenRowsSpec {
            variables: vec!["ordinal".into(), "target".into(), "field".into()],
            rows: empty
                .rows
                .into_iter()
                .map(|mut row| {
                    row.insert(1, GivenValue::String(String::new()));
                    row
                })
                .collect(),
        };
        assert!(exact(&attach) < MAX_INPUT_BYTES);
        assert!(exact(&attach) + 4 * MAX_PROVIDER_IID_BYTES as usize > MAX_INPUT_BYTES);
        let chunks =
            input_chunks_reserved(attach.clone(), false, MAX_PROVIDER_IID_BYTES as usize).unwrap();
        assert_eq!(chunks.len(), 2);
        let iid = format!("0x{}", "a".repeat(MAX_PROVIDER_IID_BYTES as usize - 2));
        for mut chunk in chunks {
            for row in &mut chunk.rows {
                row[1] = GivenValue::String(iid.clone());
            }
            assert!(exact(&chunk) <= MAX_INPUT_BYTES);
        }
        // A complete ordinal remains atomic even when its later identities grow.
        for row in &mut attach.rows {
            row[0] = GivenValue::Integer(0);
        }
        assert_eq!(
            input_chunks_reserved(attach, false, MAX_PROVIDER_IID_BYTES as usize)
                .unwrap_err()
                .code()
                .as_str(),
            "projected_batch_input_unit_byte_limit"
        );
    }

    #[test]
    fn canonical_input_split_preserves_whole_ordinals_and_refuses_an_oversized_unit() {
        let text = "x".repeat(MAX_CANONICAL_STRING_BYTES - 128);
        let split = input_chunks(
            rows((0..5).map(|ordinal| (ordinal, text.clone())).collect()),
            false,
        )
        .unwrap();
        assert_eq!(
            split.iter().map(|rows| rows.rows.len()).collect::<Vec<_>>(),
            [4, 1]
        );
        assert!(split.iter().all(|rows| exact(rows) <= MAX_INPUT_BYTES));
        let error =
            input_chunks(rows((0..5).map(|_| (0, text.clone())).collect()), false).unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "projected_batch_input_unit_byte_limit"
        );
    }
}
