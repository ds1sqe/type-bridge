//! Borrowed terminals over one recording; only staged terminals are compiled.
use super::*;
use type_bridge_contract::query_plan::v3::{
    BuildSelection, BuiltPlans, QueryPlanV3, ThingDocument,
};
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::match_request::{
    DescriptorId, PlanFetchStack, build_bulk_plan_fetch_stack, build_plan_fetch_stack,
};

pub(super) enum Selection {
    Matched,
    Count,
    Exists,
    Model {
        document: ThingDocument,
        declared: TypeId,
    },
    Rows {
        columns: Vec<BindingId>,
        models: Vec<(BindingId, TypeId)>,
    },
}
pub(super) struct HeldSelection {
    selection: Selection,
    compiled: std::cell::OnceCell<Arc<CompiledSelection>>,
}

pub(crate) enum SelectionProgram {
    Raw(Box<BuiltPlans>),
    Models(PlanFetchStack),
}
pub(crate) struct CompiledSelection {
    pub(crate) program: SelectionProgram,
    pub(crate) package: Arc<crate::abi::SchemaPackageState>,
    pub(crate) registry: Option<Arc<DescriptorRegistry>>,
    pub(crate) view_layouts: Option<Arc<type_bridge_contract::projection::c_view::CViewLayouts>>,
    pub(crate) model: Option<TypeId>,
    pub(crate) model_slots: Vec<(BindingId, TypeId)>,
    pub(crate) rows: bool,
}
impl AsRef<[QueryPlanV3]> for CompiledSelection {
    fn as_ref(&self) -> &[QueryPlanV3] {
        match &self.program {
            SelectionProgram::Raw(plans) => plans.plans(),
            SelectionProgram::Models(stack) => stack.plans(),
        }
    }
}
impl AsRef<PlanFetchStack> for CompiledSelection {
    fn as_ref(&self) -> &PlanFetchStack {
        match &self.program {
            SelectionProgram::Models(stack) => stack,
            SelectionProgram::Raw(_) => unreachable!("only model programs stage a fetch group"),
        }
    }
}
impl TypeBridgePlanBuilder {
    pub(super) fn hold(&mut self, selection: Selection) -> Result<u32, TypeBridgeStatus> {
        self.require_open()?;
        if let Some(error) = &self.error {
            return Err(crate::execution_diagnostic::status_for(error));
        }
        let id =
            u32::try_from(self.selections.len()).map_err(|_| TypeBridgeStatus::ResourceLimit)?;
        self.selections.push(HeldSelection {
            selection,
            compiled: std::cell::OnceCell::new(),
        });
        Ok(id)
    }

    pub(crate) fn finish_selection(
        &mut self,
        id: u32,
    ) -> Result<Arc<CompiledSelection>, SdkExecutionDiagnostic> {
        let selected = self.selections.get(id as usize).ok_or_else(id_unknown)?;
        let compiled = match selected.compiled.get() {
            Some(compiled) => Arc::clone(compiled),
            None => {
                let compiled = self.compile_selection(&selected.selection)?;
                Arc::clone(selected.compiled.get_or_init(|| compiled))
            }
        };
        self.frozen = true;
        Ok(compiled)
    }

    fn compile_selection(
        &self,
        selection: &Selection,
    ) -> Result<Arc<CompiledSelection>, SdkExecutionDiagnostic> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let package = self.package.as_ref().ok_or_else(|| {
            invalid(
                "plan_context_missing",
                "Typed staging requires the generated package_plan_open constructor",
            )
        })?;
        let selected = match selection {
            Selection::Matched => BuildSelection::Matched,
            Selection::Count => BuildSelection::Count,
            Selection::Exists => BuildSelection::Exists,
            Selection::Model { document, .. } => BuildSelection::Model(document),
            Selection::Rows { columns, .. } => BuildSelection::Rows(columns),
        };
        let plans = self
            .core
            .build_selected_plans(
                package._authority.declared_schema(),
                package
                    ._authority
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
                selected,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .map_err(|error| contract_error(&error, "The selected plan program is invalid"))?;
        let model_slots = match selection {
            Selection::Model { document, declared } => vec![(document.thing, declared.clone())],
            Selection::Rows { models, .. } => models.clone(),
            _ => Vec::new(),
        };
        let (program, registry) = if !model_slots.is_empty() {
            let registry = self
                .registry
                .get_or_init(|| {
                    package
                        .installed_projection
                        .match_registry()
                        .map(Arc::new)
                        .map_err(|_| {
                            invalid(
                                "plan_model_registry_invalid",
                                "The captured model registry is invalid",
                            )
                        })
                })
                .clone()?;
            let slots = model_slots
                .iter()
                .map(|(binding, declared)| {
                    let kind = match declared.kind() {
                        TypeKind::Entity => "entity",
                        TypeKind::Relation => "relation",
                        _ => return Err(id_unknown()),
                    };
                    Ok((
                        *binding,
                        DescriptorId::new(format!("{kind}:{}", declared.label().as_str())),
                    ))
                })
                .collect::<Result<Vec<_>, SdkExecutionDiagnostic>>()?;
            let stack = if matches!(selection, Selection::Rows { .. }) {
                let [plan] = plans.plans() else {
                    return Err(invalid(
                        "plan_batch_split",
                        "Tuple model columns require one authored read selection",
                    ));
                };
                build_plan_fetch_stack(&registry, plan, &slots)
            } else {
                build_bulk_plan_fetch_stack(
                    &registry,
                    plans,
                    slots.into_iter().next().expect("one model slot"),
                )
            }
            .map_err(|error| contract_error(&error, "The selected model projection is invalid"))?;
            (SelectionProgram::Models(stack), Some(registry))
        } else {
            (SelectionProgram::Raw(Box::new(plans)), None)
        };
        Ok(Arc::new(CompiledSelection {
            program,
            package: Arc::clone(package),
            registry,
            view_layouts: self.view_layouts.clone(),
            model: match selection {
                Selection::Model { declared, .. } => Some(declared.clone()),
                _ => None,
            },
            model_slots,
            rows: matches!(selection, Selection::Rows { .. }),
        }))
    }
}

/// Hold a terminal without compiling, freezing, or copying the recording.
/// Kinds: 0 matched, 1 count, 2 exists, 3 model. Model arguments are unused for other kinds.
///
/// # Safety
/// The recording is live and exclusively borrowed. The writable output cannot
/// overlap the recording or any captured package bytes and may be unaligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_hold(
    builder: *mut TypeBridgePlanBuilder,
    kind: u32,
    binding: u32,
    model_ordinal: u32,
    out_selection: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract; fences and clears the output first.
    if let Err(status) = unsafe { enter(builder, Some(out_selection), &[]) } {
        return status;
    }
    guarded(|| {
        // SAFETY: the live recording is exclusively borrowed.
        let builder = unsafe { &mut *builder };
        if let Err(status) = builder.require_open() {
            return status;
        }
        if let Some(error) = &builder.error {
            return crate::execution_diagnostic::status_for(error);
        }
        let selection = match kind {
            0 => Selection::Matched,
            1 => Selection::Count,
            2 => Selection::Exists,
            3 => {
                let Some(binding) = builder
                    .binding(binding)
                    .filter(|id| builder.core.contains_binding(*id))
                else {
                    builder.fail(id_unknown());
                    return TypeBridgeStatus::InvalidArgument;
                };
                let Some(declared) = builder
                    .models
                    .get(model_ordinal as usize)
                    .and_then(Option::as_ref)
                    .and_then(|model| builder.types.get(model.type_id as usize))
                    .and_then(Option::as_ref)
                else {
                    builder.fail(id_unknown());
                    return TypeBridgeStatus::InvalidArgument;
                };
                let Some(model) = builder
                    .package
                    .as_ref()
                    .and_then(|package| package._projection.models().get(declared))
                else {
                    builder.fail(id_unknown());
                    return TypeBridgeStatus::InvalidArgument;
                };
                let roles = model
                    .query_tokens()
                    .roles()
                    .keys()
                    .map(|role| DocumentRole::new(role.clone(), false))
                    .collect();
                Selection::Model {
                    document: ThingDocument::new(binding, roles),
                    declared: declared.clone(),
                }
            }
            _ => {
                builder.fail(constant_invalid());
                return TypeBridgeStatus::InvalidArgument;
            }
        };
        match builder.hold(selection) {
            Ok(id) => {
                // SAFETY: output is fenced and writable.
                unsafe { out_selection.write_unaligned(id) };
                TypeBridgeStatus::Ok
            }
            Err(status) => status,
        }
    })
}

pub(crate) enum AuthoredAnswers {
    Raw(
        Vec<
            type_bridge_orm::session::staged::Pending<type_bridge_orm::session::staged::PlanAnswer>,
        >,
    ),
    Models(
        type_bridge_orm::session::staged::Pending<
            type_bridge_orm::match_request::OwnedPlanFetchResult,
        >,
    ),
}
pub(crate) struct AuthoredPending {
    pub(crate) source: Arc<CompiledSelection>,
    pub(crate) answers: AuthoredAnswers,
    pub(crate) origin: type_bridge_orm::ProjectedQueryOrigin,
    pub(crate) rows_result: std::sync::OnceLock<
        Result<Arc<crate::plan_results::rows::RowsResultState>, SdkExecutionDiagnostic>,
    >,
    pub(crate) model_result: std::sync::OnceLock<
        Result<Arc<crate::plan_results::ModelResultState>, SdkExecutionDiagnostic>,
    >,
    merged: std::sync::OnceLock<
        Result<
            Arc<type_bridge_orm::session::staged::PlanAnswer>,
            type_bridge_orm::session::staged::PendingError,
        >,
    >,
}
impl AuthoredPending {
    pub(crate) fn new(
        source: Arc<CompiledSelection>,
        answers: AuthoredAnswers,
        origin: type_bridge_orm::ProjectedQueryOrigin,
    ) -> Self {
        Self {
            source,
            answers,
            origin,
            model_result: std::sync::OnceLock::new(),
            rows_result: std::sync::OnceLock::new(),
            merged: std::sync::OnceLock::new(),
        }
    }
    pub(crate) fn selection_plan(&self) -> &QueryPlanV3 {
        match &self.source.program {
            SelectionProgram::Raw(plans) => plans.projection_plan(),
            SelectionProgram::Models(stack) => stack.selection_plan(),
        }
    }
    pub(crate) fn answer(
        &self,
    ) -> Result<
        Arc<type_bridge_orm::session::staged::PlanAnswer>,
        type_bridge_orm::session::staged::PendingError,
    > {
        use type_bridge_orm::session::staged::{PendingError, merge_plan_answers};
        match &self.answers {
            AuthoredAnswers::Models(answer) => Ok(answer.get()?.shared_selection()),
            AuthoredAnswers::Raw(pending) => {
                let answers = pending
                    .iter()
                    .map(|answer| answer.get())
                    .collect::<Result<Vec<_>, _>>()?;
                self.merged
                    .get_or_init(|| {
                        let SelectionProgram::Raw(plans) = &self.source.program else {
                            unreachable!("raw pending uses raw source")
                        };
                        let correlations = pending
                            .iter()
                            .map(|answer| answer.correlation())
                            .collect::<Result<Vec<_>, _>>()?;
                        merge_plan_answers(plans, &answers, &correlations)
                            .map_err(|error| PendingError::InvalidPlan(Box::new(error)))
                    })
                    .clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_terminals_compile_once_in_any_order_and_keep_the_source_alive() {
        let package = crate::entity_crud::tests::package("held");
        let mut recording = TypeBridgePlanBuilder::new();
        recording.package = Some(Arc::clone(package.state()));
        let model = recording.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("person".into()));
        recording.thing(model, PlanBuilder::exact);
        let count = recording.hold(Selection::Count).unwrap();
        let exists = recording.hold(Selection::Exists).unwrap();
        let unused = recording.hold(Selection::Matched).unwrap();
        assert!(
            recording
                .selections
                .iter()
                .all(|held| held.compiled.get().is_none())
        );
        let exists_first = recording.finish_selection(exists).unwrap();
        let count_first = recording.finish_selection(count).unwrap();
        assert!(Arc::ptr_eq(
            &exists_first,
            &recording.finish_selection(exists).unwrap()
        ));
        assert!(Arc::ptr_eq(
            &count_first,
            &recording.finish_selection(count).unwrap()
        ));
        assert!(
            recording.selections[unused as usize]
                .compiled
                .get()
                .is_none()
        );
        assert!(recording.hold(Selection::Matched).is_err());
        assert!(Arc::ptr_eq(
            &count_first,
            &recording.finish_selection(count).unwrap()
        ));
        assert!(recording.error.is_none());
        let bytes = AsRef::<[QueryPlanV3]>::as_ref(count_first.as_ref())[0]
            .canonical_bytes()
            .unwrap();
        drop(recording);
        assert_eq!(
            bytes,
            AsRef::<[QueryPlanV3]>::as_ref(count_first.as_ref())[0]
                .canonical_bytes()
                .unwrap()
        );
        assert_eq!(
            AsRef::<[QueryPlanV3]>::as_ref(exists_first.as_ref())[0].operation(),
            type_bridge_contract::query_plan::v3::Operation::Exists
        );
    }
    #[test]
    fn typed_stage_reuses_original_source_and_pending_outlives_recording() {
        use crate::entity_crud::tests::{COMMIT_SUCCESS, Response, fixture};
        use crate::staged::*;
        use type_bridge_orm::session::backend::QueryResult;
        let response = || Response::Result(QueryResult::Rows(vec![serde_json::json!({"row": 0})]));
        let fixture = fixture(vec![response(), response(), response()], COMMIT_SUCCESS);
        let mut recording = TypeBridgePlanBuilder::new();
        recording.package = Some(Arc::clone(fixture.package.state()));
        let model = recording.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("person".into()));
        recording.thing(model, PlanBuilder::exact);
        let count = recording.hold(Selection::Count).unwrap();
        let exists = recording.hold(Selection::Exists).unwrap();
        let unused = recording.hold(Selection::Matched).unwrap();
        let (mut transaction, mut diagnostic) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: every input and output is a live, distinct test-owned object.
        unsafe {
            assert_eq!(
                type_bridge_database_transaction(
                    &*fixture.database,
                    TYPE_BRIDGE_TRANSACTION_READ,
                    &mut transaction,
                    &mut diagnostic
                ),
                TypeBridgeStatus::Ok
            );
            let mut pending = [ptr::null_mut(); 3];
            for (slot, selected) in pending.iter_mut().zip([exists, count, count]) {
                assert_eq!(
                    type_bridge_staged_transaction_stage_selection(
                        transaction,
                        &mut recording,
                        selected,
                        slot,
                        &mut diagnostic
                    ),
                    TypeBridgeStatus::Ok
                );
            }
            assert!(recording.frozen);
            assert!(
                recording.selections[unused as usize]
                    .compiled
                    .get()
                    .is_none()
            );
            let source = |pending: *mut TypeBridgePending| {
                let pending = &*pending;
                let crate::staged_answers::PendingSource::Authored(pending) =
                    &pending.state.pending
                else {
                    panic!("typed source")
                };
                Arc::clone(&pending.source)
            };
            assert!(Arc::ptr_eq(&source(pending[1]), &source(pending[2])));
            assert_eq!(fixture.state.query_count(), 0);
            assert_eq!(
                type_bridge_plan_builder_exists(&mut recording),
                TypeBridgeStatus::InvalidArgument
            );
            let mut exists_value = true;
            assert_eq!(
                crate::staged_terminals::type_bridge_exists_pending_result(
                    pending[0],
                    &mut exists_value
                ),
                TypeBridgeStatus::InUse
            );
            assert!(!exists_value);
            drop(recording);
            assert_eq!(
                type_bridge_staged_transaction_commit(transaction, &mut diagnostic),
                TypeBridgeStatus::Ok
            );
            assert!(diagnostic.is_null());
            let mut exists_value = false;
            let mut count_value = 0;
            assert_eq!(
                crate::staged_terminals::type_bridge_exists_pending_result(
                    pending[0],
                    &mut exists_value
                ),
                TypeBridgeStatus::Ok
            );
            assert!(exists_value);
            assert_eq!(
                crate::staged_terminals::type_bridge_count_pending_result(
                    pending[1],
                    &mut count_value
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count_value, 1);
            assert_eq!(
                crate::staged_terminals::type_bridge_matched_pending_result(
                    pending[1],
                    &mut count_value
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count_value, 1);
            assert_eq!(
                crate::staged_terminals::type_bridge_exists_pending_result(
                    pending[1],
                    &mut exists_value
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert!(!exists_value);
            assert_eq!(
                crate::staged_terminals::type_bridge_count_pending_result(
                    pending[0],
                    &mut count_value
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(count_value, 0);
            assert_eq!(
                crate::staged_terminals::type_bridge_exists_pending_result(
                    pending[0],
                    pending[0].cast()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            type_bridge_staged_transaction_close(&mut transaction);
            for (pending, expected) in pending.iter_mut().zip([
                TypeBridgePlanAnswerKind::Exists,
                TypeBridgePlanAnswerKind::Count,
                TypeBridgePlanAnswerKind::Count,
            ]) {
                let (mut kind, mut value) = (0, 0);
                assert_eq!(
                    type_bridge_pending_answer(*pending, &mut kind, &mut value),
                    TypeBridgeStatus::Ok
                );
                assert_eq!((kind, value), (expected as u32, 1));
                type_bridge_pending_free(pending);
            }
            assert_eq!(fixture.state.query_count(), 3);
        }
    }
}

#[cfg(test)]
#[path = "selection_review_tests.rs"]
mod review_tests;
