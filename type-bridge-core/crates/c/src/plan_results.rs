//! Owned model results and borrowed field views over the core's published proof.
pub(crate) mod rows;
mod views;

use std::collections::BTreeMap;
use std::ffi::CString;
use std::mem::size_of;
use std::ptr;
use std::sync::{Arc, OnceLock};

use crate::abi::{SchemaPackageState, TypeBridgeStatus, guarded};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{
    BorrowedRangeVisitor, DirectOutputPreflight, RetainedRangeIndex, direct_output_preflight,
};
use crate::plan_builder::selections::{AuthoredAnswers, AuthoredPending, CompiledSelection};
use crate::projected_token::TypeBridgeProjectedTokenV1;
use crate::staged::{TypeBridgePending, invalid, prepare_handle_outputs};
use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::projection::ProjectedTokenIdentity;
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::match_request::OwnedPlanFetchResult;
use type_bridge_orm::{
    ProjectedAttributeValue, ProjectedQuerySlotValue, ProjectedQueryValue, ProjectedRolePlayer,
    ProjectedThing,
};

/// Owned snapshot retaining one published model proof and its borrowed views.
pub struct TypeBridgePlanResult {
    state: Arc<ModelResultState>,
}
pub(crate) struct ModelResultState {
    source: Arc<CompiledSelection>,
    proof: Arc<OwnedPlanFetchResult>,
    _projected: ProjectedQueryValue,
    models: Vec<Vec<ModelViewData>>,
    views: Option<views::ViewStorage>,
    retained_index: OnceLock<Result<RetainedRangeIndex, TypeBridgeStatus>>,
}

/// Borrowed model view. This object and all returned strings borrow a result owner.
/// Never free it; freeing the result invalidates this and every nested view.
struct ModelViewData {
    package: Arc<SchemaPackageState>,
    type_id: TypeId,
    iid: CString,
    deleted: bool,
    fields: BTreeMap<OwnsFactId, Vec<Scalar>>,
    roles: BTreeMap<RoleId, Vec<ModelViewData>>,
}
enum Scalar {
    Long(i64),
    Double(f64),
    Boolean(bool),
    Text(CString),
}
impl Scalar {
    fn new(value: &CanonicalValue) -> Result<Self, SdkExecutionDiagnostic> {
        Ok(match value {
            CanonicalValue::Long(value) => Self::Long(*value),
            CanonicalValue::Double(value) => Self::Double(value.get()),
            CanonicalValue::Boolean(value) => Self::Boolean(*value),
            CanonicalValue::String(value) => Self::Text(c_string(value.as_str().as_bytes())?),
            CanonicalValue::Decimal(value) => Self::Text(c_string(value.as_str().as_bytes())?),
            value => {
                let text = crate::projected_value::InlineCanonicalText::new(value)
                    .expect("temporal scalar");
                Self::Text(c_string(text.as_bytes())?)
            }
        })
    }
}
fn c_string(bytes: &[u8]) -> Result<CString, SdkExecutionDiagnostic> {
    CString::new(bytes).map_err(|_| {
        invalid(
            "c_plan_view_embedded_nul",
            "A C model-view string contains an embedded NUL",
        )
    })
}
fn fields(
    values: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
) -> Result<BTreeMap<OwnsFactId, Vec<Scalar>>, SdkExecutionDiagnostic> {
    values
        .iter()
        .map(|(field, values)| {
            Ok((
                field.clone(),
                values
                    .iter()
                    .map(|value| Scalar::new(value.value()))
                    .collect::<Result<_, _>>()?,
            ))
        })
        .collect()
}
impl ModelViewData {
    fn root(
        package: &Arc<SchemaPackageState>,
        thing: &ProjectedThing,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Ok(Self {
            package: Arc::clone(package),
            type_id: thing.type_id().clone(),
            iid: c_string(thing.iid().as_bytes())?,
            deleted: thing.is_deleted(),
            fields: fields(thing.fields())?,
            roles: thing
                .roles()
                .iter()
                .map(|(role, players)| {
                    Ok((
                        role.clone(),
                        players
                            .iter()
                            .map(|player| Self::player(package, player))
                            .collect::<Result<_, _>>()?,
                    ))
                })
                .collect::<Result<_, SdkExecutionDiagnostic>>()?,
        })
    }
    fn player(
        package: &Arc<SchemaPackageState>,
        player: &ProjectedRolePlayer,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let mut values = fields(player.fields())?;
        for (field, value) in player.keys() {
            if !values.contains_key(field) {
                values.insert(field.clone(), vec![Scalar::new(value.value())?]);
            }
        }
        Ok(Self {
            package: Arc::clone(package),
            type_id: player.type_id().clone(),
            iid: c_string(player.iid().as_bytes())?,
            deleted: false,
            fields: values,
            roles: BTreeMap::new(),
        })
    }
    fn fence(&self, outputs: &dyn BorrowedRangeVisitor) -> Result<(), TypeBridgeStatus> {
        outputs.check_bytes((self as *const Self).cast(), size_of::<Self>())?;
        outputs.check_package_borrowed_ranges(&self.package)?;
        outputs.check_bytes(self.iid.as_ptr().cast(), self.iid.as_bytes_with_nul().len())?;
        for values in self.fields.values() {
            for value in values {
                if let Scalar::Text(text) = value {
                    outputs.check_bytes(text.as_ptr().cast(), text.as_bytes_with_nul().len())?;
                }
            }
        }
        for players in self.roles.values() {
            for player in players {
                player.fence(outputs)?;
            }
        }
        Ok(())
    }
}
impl ModelResultState {
    fn build(
        pending: &AuthoredPending,
        proof: Arc<OwnedPlanFetchResult>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let (projected, _) = pending.origin.clone().materialize_borrowed_with_budget(
            &pending.source.package.installed_projection,
            pending
                .source
                .registry
                .as_ref()
                .expect("model source retains registry"),
            proof.request(),
            proof.result(),
            Default::default(),
            &Default::default(),
            None,
        )?;
        let ProjectedQueryValue::Rows { rows } = &projected else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        let models = rows
            .iter()
            .map(|row| {
                row.slots()
                    .iter()
                    .map(|slot| {
                        let ProjectedQuerySlotValue::One(thing) = slot.value() else {
                            return Err(SdkExecutionDiagnostic::internal_failure());
                        };
                        ModelViewData::root(&pending.source.package, thing)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let views = pending
            .source
            .view_layouts
            .as_ref()
            .map(|layouts| {
                let declarations = proof
                    .model_bindings()
                    .iter()
                    .map(|binding| {
                        pending
                            .source
                            .model_slots
                            .iter()
                            .find(|(model_binding, _)| model_binding == binding)
                            .map(|(_, declared)| declared.clone())
                            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                views::ViewStorage::build_columns(layouts, &declarations, &models)
            })
            .transpose()?;
        Ok(Self {
            source: Arc::clone(&pending.source),
            proof,
            _projected: projected,
            models,
            views,
            retained_index: OnceLock::new(),
        })
    }
    pub(crate) fn fence(&self, outputs: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        outputs.check_package_borrowed_ranges(&self.source.package)?;
        if let Some(views) = &self.views {
            views.fence(outputs)?;
        }
        self.retained_index
            .get_or_init(|| {
                RetainedRangeIndex::collect(|collector| {
                    for row in &self.models {
                        for model in row {
                            model.fence(collector)?;
                        }
                    }
                    Ok(())
                })
            })
            .as_ref()
            .map_err(|status| *status)?
            .check(outputs)
    }
}
impl AuthoredPending {
    fn model_result(&self) -> Result<Arc<ModelResultState>, SdkExecutionDiagnostic> {
        let AuthoredAnswers::Models(pending) = &self.answers else {
            return Err(invalid(
                "plan_result_not_models",
                "This terminal does not select models",
            ));
        };
        let proof = pending
            .get()
            .map_err(|error| crate::staged::pending_diagnostic(&error))?;
        self.model_result
            .get_or_init(|| ModelResultState::build(self, proof).map(Arc::new))
            .clone()
    }
}

/// Return an owned model result snapshot. Repeated calls share one materialization.
/// # Safety
/// Inputs are live; writable owner slots may be unaligned and cannot alias inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_plan_result(
    pending: *const TypeBridgePending,
    out_result: *mut *mut TypeBridgePlanResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let mut tag = None;
    let inputs = |outputs: &DirectOutputPreflight| {
        // SAFETY: input is a live cursor by contract.
        tag = unsafe {
            crate::staged_answers::check_cursor_ranges(
                outputs,
                pending.cast(),
                size_of::<TypeBridgePending>(),
            )
        }?;
        Ok(())
    };
    // SAFETY: outputs are fenced before either owner slot is cleared.
    if let Err(status) = unsafe { prepare_handle_outputs(out_result, out_diagnostics, inputs) } {
        return status;
    }
    if tag != Some(crate::staged_answers::PENDING_TAG) {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: tag and complete cursor were checked above.
        let crate::staged_answers::PendingSource::Authored(pending) =
            &unsafe { &*pending }.state.pending
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if let Err(error) = pending.answer() {
            let status = crate::staged::answer_status(&error);
            let _ =
                return_execution_error(crate::staged::pending_diagnostic(&error), out_diagnostics);
            return status;
        }
        if pending.source.model.is_none() {
            return return_execution_error(
                invalid(
                    "plan_result_not_models",
                    "This terminal does not select a model list",
                ),
                out_diagnostics,
            );
        }
        match pending.model_result() {
            Ok(state) => {
                // SAFETY: output was fenced and initialized above.
                unsafe {
                    out_result
                        .write_unaligned(Box::into_raw(Box::new(TypeBridgePlanResult { state })))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(error, out_diagnostics),
        }
    })
}

/// Free an owned result and invalidate every borrowed view from it.
/// # Safety
/// The owner slot is writable and holds null or a live result allocated here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_result_free(result: *mut *mut TypeBridgePlanResult) {
    // SAFETY: forwarded owner-slot contract.
    unsafe { crate::staged::free_slot(result) }
}

/// Read the number of authored model rows.
/// # Safety
/// Result is live; output is writable, possibly unaligned, and disjoint from it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_result_count(
    result: *const TypeBridgePlanResult,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    // SAFETY: result and output follow the forwarded read contract.
    unsafe { result_read(result, out_count, 0, |result| Ok(result.proof.row_count())) }
}
unsafe fn result_read<T: Copy>(
    result: *const TypeBridgePlanResult,
    output: *mut T,
    empty: T,
    read: impl FnOnce(&ModelResultState) -> Result<T, TypeBridgeStatus>,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[(output.cast(), size_of::<T>())]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        if !result.is_null() {
            if let Err(status) =
                outputs.check_bytes(result.cast(), size_of::<TypeBridgePlanResult>())
            {
                return status;
            }
            // SAFETY: complete live result was fenced above.
            if let Err(status) = unsafe { &*result }.state.fence(&outputs) {
                return status;
            }
        }
        if output.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: writable output is disjoint from all result-owned inputs.
        unsafe { output.write_unaligned(empty) };
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: complete live result was fenced above.
        match read(&unsafe { &*result }.state) {
            Ok(value) => {
                unsafe { output.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(status) => status,
        }
    })
}

/// Copy one generated field-bearing root view; its pointer members borrow result.
/// # Safety
/// Result/token are live. Output names view_size writable bytes, possibly
/// unaligned, and is disjoint from every input and result-owned borrowed range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_result_view(
    result: *const TypeBridgePlanResult,
    row: usize,
    token: *const TypeBridgeProjectedTokenV1,
    out_view: *mut std::ffi::c_void,
    view_size: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[(out_view, view_size)]) {
            Ok(outputs) => outputs,
            Err(status) => return status,
        };
        if !result.is_null() {
            if let Err(status) =
                outputs.check_bytes(result.cast(), size_of::<TypeBridgePlanResult>())
            {
                return status;
            }
            // SAFETY: complete live result is fenced above.
            if let Err(status) = unsafe { &*result }.state.fence(&outputs) {
                return status;
            }
        }
        if !token.is_null()
            && let Err(status) =
                outputs.check_bytes(token.cast(), size_of::<TypeBridgeProjectedTokenV1>())
        {
            return status;
        }
        if out_view.is_null() || view_size == 0 || view_size > 65_535 {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller lends the writable extent, disjoint from all fenced inputs.
        unsafe { ptr::write_bytes(out_view.cast::<u8>(), 0, view_size) };
        if result.is_null() || token.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: both live handles were fenced above.
        let state = &unsafe { &*result }.state;
        let declared = match unsafe {
            crate::projected_token::resolve_model_token(&state.source.package, token)
        } {
            Ok(declared) => declared,
            Err(_) => return TypeBridgeStatus::InvalidArgument,
        };
        if state.source.model.as_ref() != Some(&declared) {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(views) = &state.views else {
            return TypeBridgeStatus::Unsupported;
        };
        let Some(binding) = state.proof.model_bindings().first() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let Some((row, 0)) = state.proof.model_ordinal(row, *binding) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let Some(view) = views.row(row).filter(|view| view.len() == view_size) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        // SAFETY: exact layout size and all disjoint ranges were checked above.
        unsafe { ptr::copy_nonoverlapping(view.as_ptr(), out_view.cast::<u8>(), view_size) };
        TypeBridgeStatus::Ok
    })
}

// Model views require the native C view ABI target; elsewhere
// type_bridge_plan_builder_new_with_layout refuses with c_plan_view_abi_unsupported.
#[cfg(all(
    test,
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64",
    target_endian = "little"
))]
mod tests {
    use super::*;
    use crate::entity_crud::tests::{COMMIT_SUCCESS, documents, fixture, person_document};
    use crate::plan_builder::{context::*, selections::*, *};
    use crate::staged::*;
    use std::ffi::{CStr, c_char};

    #[test]
    fn model_results_share_one_owner_cache_and_borrow_after_pending_and_recording_drop() {
        let doc = person_document("0x01");
        let mut other = person_document("0x02");
        other["attributes"]["identifier"] = serde_json::json!("grace");
        let fixture = fixture(
            (0..8)
                .map(|_| documents(vec![doc.clone(), other.clone()]))
                .collect(),
            COMMIT_SUCCESS,
        );
        let digest = type_bridge_contract::projection::c_view::CViewLayouts::for_projection(
            &fixture.package.state()._projection,
        )
        .unwrap()
        .digest();
        let mut builder = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let mut transaction = ptr::null_mut();
        let mut pending = ptr::null_mut();
        // SAFETY: all handles and owner slots are distinct, live test allocations.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_new_with_layout(
                    &fixture.package,
                    ptr::null(),
                    crate::abi::TypeBridgeByteView {
                        data: digest.as_ptr(),
                        length: digest.len()
                    },
                    &mut builder,
                    &mut diagnostics,
                ),
                TypeBridgeStatus::Ok
            );
            let mut model = 0;
            let opener = crate::abi::TypeBridgeByteView {
                data: b"test_open".as_ptr(),
                length: 9,
            };
            assert_eq!(
                type_bridge_plan_builder_type_token(builder, &fixture.token, opener, &mut model),
                TypeBridgeStatus::Ok
            );
            let mut binding = 0;
            assert_eq!(
                type_bridge_plan_builder_exact(builder, model, &mut binding),
                TypeBridgeStatus::Ok
            );
            let mut selection = 0;
            assert_eq!(
                type_bridge_plan_builder_hold(
                    builder,
                    3,
                    binding,
                    fixture.token.ordinal,
                    &mut selection
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_database_transaction(
                    &*fixture.database,
                    TYPE_BRIDGE_TRANSACTION_READ,
                    &mut transaction,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_staged_transaction_stage_selection(
                    transaction,
                    builder,
                    selection,
                    &mut pending,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            type_bridge_plan_builder_free(&mut builder);
            assert_eq!(
                type_bridge_staged_transaction_commit(transaction, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert!(diagnostics.is_null());
            type_bridge_staged_transaction_close(&mut transaction);
            let (mut first, mut second) = (ptr::null_mut(), ptr::null_mut());
            assert_eq!(
                type_bridge_pending_plan_result(pending, &mut first, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_pending_plan_result(pending, &mut second, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert!(Arc::ptr_eq(&(*first).state, &(*second).state));
            let ProjectedQueryValue::Rows { rows } = &(&*first).state._projected else {
                panic!("model rows")
            };
            let ProjectedQuerySlotValue::One(thing) = rows[0].slots()[0].value() else {
                panic!("one model")
            };
            assert!(thing.origin_carrier().is_some());
            type_bridge_pending_free(&mut pending);
            type_bridge_plan_result_free(&mut first);
            let mut count = 0;
            assert_eq!(
                type_bridge_plan_result_count(second, &mut count),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, 2);
            #[repr(C)]
            struct PersonView {
                iid: *const c_char,
                kind: u32,
                deleted: bool,
                department: *const c_char,
                has_department: bool,
                identifier: *const c_char,
            }
            assert_eq!(size_of::<PersonView>(), 40);
            let mut view = std::mem::MaybeUninit::<PersonView>::uninit();
            assert_eq!(
                type_bridge_plan_result_view(
                    second,
                    0,
                    &fixture.token,
                    view.as_mut_ptr().cast(),
                    size_of::<PersonView>()
                ),
                TypeBridgeStatus::Ok
            );
            let view = view.assume_init();
            assert_eq!(CStr::from_ptr(view.iid).to_bytes(), b"0x01");
            assert_eq!(CStr::from_ptr(view.identifier).to_bytes(), b"ada");
            assert!(view.department.is_null());
            assert!(!view.has_department);
            assert!(!view.deleted);
            assert_eq!(view.kind, fixture.token.ordinal);
            assert_eq!(
                type_bridge_plan_result_view(
                    second,
                    0,
                    &fixture.token,
                    view.identifier.cast_mut().cast(),
                    size_of::<PersonView>()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(CStr::from_ptr(view.identifier).to_bytes(), b"ada");
            let stored = (&*second).state.views.as_ref().unwrap();
            assert_eq!(
                type_bridge_plan_result_view(
                    second,
                    0,
                    &fixture.token,
                    stored.row(0).unwrap().as_ptr().cast_mut().cast(),
                    size_of::<PersonView>()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(
                type_bridge_plan_result_view(
                    second,
                    0,
                    &fixture.token,
                    stored.row(1).unwrap().as_ptr().cast_mut().cast(),
                    size_of::<PersonView>()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            let mut other_view = std::mem::MaybeUninit::<PersonView>::uninit();
            assert_eq!(
                type_bridge_plan_result_view(
                    second,
                    1,
                    &fixture.token,
                    other_view.as_mut_ptr().cast(),
                    size_of::<PersonView>()
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                CStr::from_ptr(other_view.assume_init().identifier).to_bytes(),
                b"grace"
            );
            type_bridge_plan_result_free(&mut second);
        }
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
#[path = "plan_results/view_tests.rs"]
mod view_tests;
