//! Native plan execution retains the common batch lease, event matrix and consumers.

use super::*;
use crate::plan_builders::{
    BuiltPlan,
    batch_native::{self as build, BatchDraft},
};
use crate::query_plan_exec::{self, remote_value};
use crate::session::backend::QueryResult;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::v3::{DeleteItem, DocumentRole, Stage};
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::query_remote::RemoteValue;

fn diagnostic(error: Diagnostic) -> SdkExecutionDiagnostic {
    lower_orm_error(
        &crate::plan_builders::manager::diagnostic(error),
        SdkProviderOperation::Write,
    )
}

impl BatchPlan<'_> {
    pub(super) fn complete_write_plans(
        &self,
    ) -> std::result::Result<crate::plan_builders::bulk_write::WriteProgram, SdkExecutionDiagnostic>
    {
        use crate::plan_builders::bulk_write::{WriteKind, WriteRow, WriteShape};
        use crate::plan_builders::reference::{Reference, ReferenceIdentity};
        let authority = self.installed.plan_authority().map_err(diagnostic)?;
        let fields = self
            .rows
            .iter()
            .map(|(_, row)| {
                let create =
                    row_create(row).ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                let mut fields = Vec::new();
                for (field, values) in create.fields() {
                    let declaring = self.declaring_field(field)?;
                    fields.extend(
                        values
                            .iter()
                            .map(|value| (declaring.clone(), value.value().clone())),
                    );
                }
                Ok(fields)
            })
            .collect::<std::result::Result<Vec<_>, SdkExecutionDiagnostic>>()?;
        let routes = self
            .reference_routes
            .iter()
            .map(|route| (route.key.clone(), &route.candidate_types))
            .collect::<BTreeMap<_, _>>();
        let mut references = (0..self.row_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for reference in &self.role_references {
            let route_key = if reference.reference.iid().is_some() {
                ReferenceRouteKey::Iid(
                    reference.role.clone(),
                    reference.reference.type_id().clone(),
                )
            } else {
                ReferenceRouteKey::Key(
                    reference.role.clone(),
                    reference.reference.type_id().clone(),
                    reference
                        .reference
                        .keys()
                        .first_key_value()
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?
                        .0
                        .clone(),
                )
            };
            let candidates = routes
                .get(&route_key)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let identity = if let Some(iid) = reference.reference.iid() {
                ReferenceIdentity::Iid(iid)
            } else {
                let (field, value) = reference
                    .reference
                    .keys()
                    .first_key_value()
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                ReferenceIdentity::Key {
                    attribute: field.attribute(),
                    value: value.value().clone(),
                }
            };
            references[reference.owner_ordinal].push((
                reference.role.clone(),
                Reference {
                    candidates,
                    identity,
                },
            ));
        }
        let rows = self
            .rows
            .iter()
            .zip(&fields)
            .zip(&references)
            .map(|(((_, row), fields), players)| WriteRow {
                iid: row_iid(row),
                fields,
                players,
            })
            .collect::<Vec<_>>();
        let keys = self
            .model
            .reference_read()
            .key_fields()
            .iter()
            .map(|key| self.declaring_field(key).cloned())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cleared_fields = self
            .field_routes
            .iter()
            .map(|route| self.declaring_field(&route.field).cloned())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cleared_roles = self
            .model
            .complete_read()
            .roles()
            .iter()
            .filter(|(_, projection)| projection.multiplicity().collection_mode().is_unordered())
            .map(|(role, _)| role.clone())
            .collect::<Vec<_>>();
        let document_roles = self
            .model
            .complete_read()
            .roles()
            .iter()
            .map(|(role, projection)| {
                if projection.multiplicity().collection_mode().is_unordered() {
                    DocumentRole::collected(role.clone())
                } else {
                    DocumentRole::new(role.clone(), false)
                }
            })
            .collect();
        let kind = match self.operation {
            ProjectedBatchOperation::Insert => WriteKind::Insert,
            ProjectedBatchOperation::Put => WriteKind::Put,
            ProjectedBatchOperation::Update => WriteKind::Update,
            ProjectedBatchOperation::Delete => {
                return Err(SdkExecutionDiagnostic::internal_failure());
            }
        };
        crate::plan_builders::bulk_write::write_many(
            &authority,
            kind,
            WriteShape {
                references_see_prior_writes: false,
                include_subtypes: false,
                return_documents: true,
                type_id: self.model.id(),
                keys: &keys,
                cleared_fields: &cleared_fields,
                cleared_roles: &cleared_roles,
                document_roles,
            },
            &rows,
        )
        .map_err(diagnostic)
    }

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

    fn native_delete(
        &self,
        rows: GivenRowsSpec,
    ) -> std::result::Result<BuiltPlan, SdkExecutionDiagnostic> {
        let columns = [(ValueTypeTag::Long, false), (ValueTypeTag::String, false)];
        let mut draft = BatchDraft::new(rows, &columns).map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let ordinal = draft
            .bind_input(0, "ordinal", &mut patterns)
            .map_err(diagnostic)?;
        let thing = draft.draft.bind("thing").map_err(diagnostic)?;
        patterns.push(build::isa(thing, self.model.id()).into());
        patterns.push(
            QueryPattern::Iid {
                binding: thing,
                iid: draft.inputs[1].clone(),
            }
            .into(),
        );
        draft
            .finish(
                self.installed
                    .plan_authority()
                    .map_err(diagnostic)?
                    .as_ref(),
                vec![
                    Stage::Match { patterns },
                    Stage::Delete {
                        items: vec![DeleteItem::Thing { binding: thing }],
                    },
                ],
                true,
                vec![ordinal],
            )
            .map_err(diagnostic)
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
        if plan.operation == ProjectedBatchOperation::Delete && plan.return_deleted {
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
                crate::_manager::returning::DeleteSelection::Iids(&iids),
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
        if plan.operation != ProjectedBatchOperation::Delete {
            let (mut program, mut ledger) = plan
                .write_program
                .take()
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let authority = plan.installed.plan_authority().map_err(diagnostic)?;
            let mut consumer = HydrationConsumer::new(self, plan, identity)?;
            for (_, row) in &plan.rows {
                consumer
                    .targets
                    .push(row_iid(row).unwrap_or_default().to_owned());
            }
            while let Some(step) = program.next().map_err(diagnostic)? {
                if !step.complete {
                    let mut returned = None;
                    let mut capture = |item: AnswerItem| {
                        let AnswerItem::Document(document) = item else {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                SdkExecutionDiagnostic::internal_failure(),
                            )));
                        };
                        let ordinal = document_index(&document, "ordinal", plan.row_count)
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        let iid =
                            if step.missing_iid_noop && document.get("iid").is_none() {
                                None
                            } else {
                                Some(document_string(&document, "iid").map_err(|error| {
                                    OrmError::ExecutionDiagnostic(Box::new(error))
                                })?)
                            };
                        if ordinal != step.ordinal || returned.is_some() {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(integrity_at_row(
                                "provider_identity_mismatch",
                                "A write piece repeated or changed its input ordinal",
                                plan.model.id(),
                                step.ordinal,
                            ))));
                        }
                        if let Some(iid) = &iid {
                            step.check_identity(iid).map_err(evidence)?;
                        }
                        returned = Some(iid);
                        Ok(AnswerControl::Continue)
                    };
                    dispatch(
                        lease,
                        &step.plan,
                        Reply::Mutation,
                        prepared,
                        &mut capture,
                        &mut ledger,
                        false,
                        &authority,
                    )
                    .await?;
                    let iid = returned.ok_or_else(|| {
                        integrity_at_row(
                            "provider_identity_mismatch",
                            "A write piece omitted its model identity",
                            plan.model.id(),
                            step.ordinal,
                        )
                    })?;
                    program.accept_identity(&step, iid).map_err(diagnostic)?;
                    continue;
                }
                dispatch(
                    lease,
                    &step.plan,
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
            let mut identities = BTreeSet::new();
            for (ordinal, thing) in things.iter().enumerate() {
                if !identities.insert(thing.iid()) {
                    return Err(integrity_at_row(
                        "projected_batch_mutation_identity_duplicate",
                        "A projected batch returned one identity for multiple input rows",
                        plan.model.id(),
                        ordinal,
                    ));
                }
            }
            ledger.charge_projected(&things)?;
            for (ordinal, thing) in things.iter().enumerate() {
                validate_written_role_identities(self.installed, plan, thing, ordinal)?;
            }

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
        let mutations = input_chunks(plan.delete_rows()?, false)?
            .into_iter()
            .map(|rows| plan.native_delete(rows))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let statement_count = mutations.len();
        ledger.limits = prepared.limits().for_adapter_statements(statement_count);
        ledger.charge_preflight(
            0,
            u32::try_from(statement_count).map_err(|_| complexity_overflow())?,
        )?;
        let mut identities = DeleteIdentityConsumer::new(plan.row_count)?;
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
        identities.finish(plan)?;
        prepared.check_control()?;
        Ok(ProjectedBatchResult::Deleted)
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
                    // Batch fetches leave only ordered roles uncollected.
                    if !role.collect {
                        let object = document.as_object_mut().expect("validated object");
                        let iid = object.remove(&format!("_role_{index}_iid"));
                        object.remove(&format!("_role_{index}_type"));
                        object.remove(&format!("_role_{index}_attributes"));
                        if iid.is_some_and(|value| !value.is_null()) {
                            return Err(crate::match_request::MatchError::new(
                                crate::match_request::MatchErrorCategory::ResultDecode,
                                "query_plan_hydration_ordered_role",
                                "populated ordered roles require provider fetch ordinals",
                            )
                            .into());
                        }
                        continue;
                    }
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
    preflight_statement_complexity(&query.typeql)?;
    let Some(rows) = query.rows.take() else {
        ledger.charge_bytes(
            source_bytes(&query.typeql)?,
            "projected_batch_transport_byte_limit",
        )?;
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
    };
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
