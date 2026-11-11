//! Immutable C filters over the shared projected manager path.

use std::mem::size_of;
use std::sync::Arc;

use type_bridge_contract::id::Iid;
use type_bridge_contract::projection::ProjectedTokenIdentity;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::{ProjectedManagerComparison, ProjectedManagerFilter};

use crate::abi::{
    SchemaPackageState, TypeBridgeSchemaPackage, TypeBridgeStatus, close_box, guarded,
};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted, try_reserve};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_IID, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_PROJECTED_VALUE, GENERATED_INPUT_SCHEMA_PACKAGE, direct_output_preflight,
};
use crate::iid::TypeBridgeIid;
use crate::projected_token::{
    TypeBridgeProjectedTokenV1, resolve_field_token, resolve_model_token,
};
use crate::projected_value::{
    TypeBridgeProjectedValue, invalid_brand_diagnostic, same_package_brand,
};

/// Immutable filter retaining its schema authority and copied predicate values.
pub struct TypeBridgeManagerFilter {
    pub(crate) package: Arc<SchemaPackageState>,
    pub(crate) value: ProjectedManagerFilter,
}

impl TypeBridgeManagerFilter {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)
    }
}

fn invalid(message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new("c_manager_filter_input_invalid").unwrap(),
        SdkDiagnosticMessage::new(message).unwrap(),
    )
}

fn outputs(
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[
        (out.cast(), size_of::<*mut TypeBridgeManagerFilter>()),
        (
            diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])
}

pub(crate) unsafe fn check_filter(
    preflight: &DirectOutputPreflight,
    filter: *const TypeBridgeManagerFilter,
) -> Result<(), TypeBridgeStatus> {
    if !filter.is_null() {
        preflight.check_bytes(filter.cast(), size_of::<TypeBridgeManagerFilter>())?;
        // SAFETY: caller retains the complete immutable filter owner.
        unsafe { &*filter }.check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

unsafe fn publish(
    package: Arc<SchemaPackageState>,
    result: Result<ProjectedManagerFilter, SdkExecutionDiagnostic>,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let value = match result {
        Ok(value) => value,
        Err(error) => return return_execution_error(error, diagnostics),
    };
    let slot = match ReservedBox::try_new(AllocationSite::ManagerFilterHandle) {
        Ok(slot) => slot,
        Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
    };
    // SAFETY: caller preflight fenced and initialized both outputs.
    unsafe {
        out.write_unaligned(Box::into_raw(
            slot.initialize(TypeBridgeManagerFilter { package, value }),
        ))
    };
    TypeBridgeStatus::Ok
}

/// Validate a generated manager's exact model before dispatching a terminal.
#[unsafe(export_name = "type_bridge_manager_filter_validate_model")]
pub(crate) unsafe extern "C" fn filter_validate_model(
    filter: *const TypeBridgeManagerFilter,
    owner: *const TypeBridgeManagerFilter,
    model: *const TypeBridgeProjectedTokenV1,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[(
            diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        )]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains the complete filter and token owners.
        if let Err(status) = unsafe { check_filter(&outputs, filter) }
            .and_then(|()| unsafe { check_filter(&outputs, owner) })
            .and_then(|()| outputs.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast()))
        {
            return status;
        }
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: writable diagnostic slot is disjoint from every input.
        unsafe { diagnostics.write_unaligned(std::ptr::null_mut()) };
        if filter.is_null() || owner.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let filter = unsafe { &*filter };
        let owner = unsafe { &*owner };
        if !same_package_brand(&filter.package, &owner.package) {
            return return_execution_error(invalid_brand_diagnostic(), diagnostics);
        }
        match unsafe { resolve_model_token(&filter.package, model) } {
            Ok(model) if &model == filter.value.model() && &model == owner.value.model() => {
                TypeBridgeStatus::Ok
            }
            Ok(_) => return_execution_error(
                invalid("The filter does not belong to this exact manager model"),
                diagnostics,
            ),
            Err(error) => return_execution_error(error, diagnostics),
        }
    })
}

/// Start an explicit all-matches filter for one exact generated model.
#[unsafe(export_name = "type_bridge_manager_filter_open")]
pub(crate) unsafe extern "C" fn filter_open(
    package: *const TypeBridgeSchemaPackage,
    model: *const TypeBridgeProjectedTokenV1,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight = match outputs(out, diagnostics) {
            Ok(value) => value,
            Err(status) => return status,
        };
        if let Err(status) = preflight
            .check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())
            .and_then(|()| {
                preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast())
            })
        {
            return status;
        }
        if !package.is_null() {
            // SAFETY: complete package owner was fenced above.
            if let Err(status) =
                preflight.check_package_borrowed_ranges(unsafe { &*package }.state())
            {
                return status;
            }
        }
        // SAFETY: outputs are pairwise and input disjoint.
        if let Err(status) = unsafe { initialize_execution_outputs(out, diagnostics) } {
            return status;
        }
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the preflighted package/token.
        let package = unsafe { &*package }.state();
        let result = unsafe { resolve_model_token(package, model) }.and_then(|model| {
            ProjectedManagerFilter::try_new(&package.installed_projection, model)
        });
        // SAFETY: initialized writable result slot.
        unsafe { publish(Arc::clone(package), result, out, diagnostics) }
    })
}

unsafe fn iid_filter(
    source: *const TypeBridgeManagerFilter,
    iids: *const *const TypeBridgeIid,
    length: usize,
    single: bool,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight = match outputs(out, diagnostics) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: source is retained for this immutable operation.
        if let Err(status) = unsafe { check_filter(&preflight, source) } {
            return status;
        }
        let Some(bytes) = length.checked_mul(size_of::<*const TypeBridgeIid>()) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if length != 0 {
            if iids.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            if let Err(status) = preflight.check_bytes(iids.cast(), bytes) {
                return status;
            }
            for index in 0..length {
                // SAFETY: caller provides the complete retained pointer table.
                let iid = unsafe { iids.add(index).read_unaligned() };
                if let Err(status) = preflight.check_object_kind(GENERATED_INPUT_IID, iid.cast()) {
                    return status;
                }
            }
        }
        // SAFETY: the source, table and each IID owner are disjoint from outputs.
        if let Err(status) = unsafe { initialize_execution_outputs(out, diagnostics) } {
            return status;
        }
        if source.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: preflight fenced the live immutable owner.
        let source = unsafe { &*source };
        if let Err(error) = type_bridge_orm::ProjectedBatch::validate_binding_row_count(length) {
            return return_execution_error(error, diagnostics);
        }
        let installed = &source.package.installed_projection;
        if single {
            // SAFETY: preflight fenced the one-element table and its IID owner.
            let iid = unsafe { iids.read_unaligned() };
            if iid.is_null() {
                return return_execution_error(
                    invalid("An IID filter requires a live IID owner"),
                    diagnostics,
                );
            }
            let iid = Iid::new(unsafe { &*iid }.as_str()).expect("C IID owner is validated");
            let result = source.value.try_and_iid(installed, &iid);
            // SAFETY: outputs were initialized and fenced.
            return unsafe { publish(Arc::clone(&source.package), result, out, diagnostics) };
        }
        let mut copied = Vec::new();
        if try_reserve(&mut copied, length, AllocationSite::ManagerFilterIids).is_err() {
            return return_execution_error(allocation_exhausted(), diagnostics);
        }
        for index in 0..length {
            // SAFETY: preflight fenced the pointer table and each owner.
            let iid = unsafe { iids.add(index).read_unaligned() };
            if iid.is_null() {
                return return_execution_error(
                    invalid("An IID filter requires live IID owners"),
                    diagnostics,
                );
            }
            copied.push(Iid::new(unsafe { &*iid }.as_str()).expect("C IID owner is validated"));
        }
        let result = source.value.try_and_owned_iids(installed, copied);
        // SAFETY: outputs were initialized and fenced.
        unsafe { publish(Arc::clone(&source.package), result, out, diagnostics) }
    })
}

/// Append an exact validated IID predicate without changing the source filter.
#[unsafe(export_name = "type_bridge_manager_filter_iid")]
pub(crate) unsafe extern "C" fn filter_iid(
    source: *const TypeBridgeManagerFilter,
    iid: *const TypeBridgeIid,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the local pointer table lives through complete delegated preflight/copy.
    unsafe { iid_filter(source, &iid, 1, true, out, diagnostics) }
}

/// Append an IID list. An empty list matches none and needs no pointer table.
#[unsafe(export_name = "type_bridge_manager_filter_iids")]
pub(crate) unsafe extern "C" fn filter_iids(
    source: *const TypeBridgeManagerFilter,
    iids: *const *const TypeBridgeIid,
    length: usize,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the source and list through delegated preflight/copy.
    unsafe { iid_filter(source, iids, length, false, out, diagnostics) }
}

/// Append one typed field comparison using the shared manager predicate rules.
#[unsafe(export_name = "type_bridge_manager_filter_compare")]
pub(crate) unsafe extern "C" fn filter_compare(
    source: *const TypeBridgeManagerFilter,
    field: *const TypeBridgeProjectedTokenV1,
    comparison: u32,
    value: *const TypeBridgeProjectedValue,
    out: *mut *mut TypeBridgeManagerFilter,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight = match outputs(out, diagnostics) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: source owner remains immutable through construction.
        if let Err(status) = unsafe { check_filter(&preflight, source) }
            .and_then(|()| {
                preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, field.cast())
            })
            .and_then(|()| {
                preflight.check_object_kind(GENERATED_INPUT_PROJECTED_VALUE, value.cast())
            })
        {
            return status;
        }
        if !value.is_null() {
            // SAFETY: complete projected value owner was fenced above.
            if let Err(status) = unsafe { &*value }.check_borrowed_ranges(&preflight) {
                return status;
            }
        }
        // SAFETY: all inputs were fenced before output initialization.
        if let Err(status) = unsafe { initialize_execution_outputs(out, diagnostics) } {
            return status;
        }
        if source.is_null() || value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: retained immutable handles were fenced above.
        let (source, value) = unsafe { (&*source, &*value) };
        if !same_package_brand(&source.package, &value.package) {
            return return_execution_error(invalid_brand_diagnostic(), diagnostics);
        }
        let comparison = match comparison {
            1 => ProjectedManagerComparison::Eq,
            2 => ProjectedManagerComparison::Ne,
            3 => ProjectedManagerComparison::Lt,
            4 => ProjectedManagerComparison::Lte,
            5 => ProjectedManagerComparison::Gt,
            6 => ProjectedManagerComparison::Gte,
            _ => {
                return return_execution_error(
                    invalid("Unknown manager comparison tag"),
                    diagnostics,
                );
            }
        };
        // SAFETY: the nominal field token was fenced above.
        let result =
            unsafe { resolve_field_token(&source.package, field) }.and_then(|(owner, field)| {
                source.value.try_and(
                    &source.package.installed_projection,
                    &ProjectedTokenIdentity::Field { owner, field },
                    comparison,
                    &value.value,
                )
            });
        // SAFETY: outputs were initialized and fenced.
        unsafe { publish(Arc::clone(&source.package), result, out, diagnostics) }
    })
}

/// Close one filter; derived filters retain independent immutable state.
#[unsafe(export_name = "type_bridge_manager_filter_close")]
pub(crate) unsafe extern "C" fn filter_close(
    filter: *mut *mut TypeBridgeManagerFilter,
) -> TypeBridgeStatus {
    // SAFETY: caller supplies the unique owning slot or null.
    unsafe { close_box(filter) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{COMMIT_SUCCESS, close_diagnostics, diagnostic, fixture};
    use std::ptr;

    fn open(fixture: &crate::entity_crud::tests::Fixture) -> *mut TypeBridgeManagerFilter {
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: fixture owns the complete package/token and disjoint outputs.
        assert_eq!(
            unsafe {
                filter_open(
                    &fixture.package,
                    &fixture.token,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        result
    }

    #[test]
    fn manager_model_validation_rejects_wrong_model_and_output_alias() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut filter = open(&fixture);
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe { filter_validate_model(filter, filter, &fixture.token, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        let other = crate::entity_crud::tests::model_token(
            &fixture.package,
            type_bridge_contract::id::TypeId::new(
                type_bridge_contract::id::TypeKind::Relation,
                "membership",
            )
            .unwrap(),
        );
        assert_eq!(
            unsafe { filter_validate_model(filter, filter, &other, &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { filter_validate_model(filter, filter, &fixture.token, filter.cast()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { filter_validate_model(filter, filter, &fixture.token, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        let foreign = crate::entity_crud::tests::package("foreignfilter");
        let foreign_model = crate::entity_crud::tests::model_token(
            &foreign,
            type_bridge_contract::id::TypeId::new(
                type_bridge_contract::id::TypeKind::Entity,
                "person",
            )
            .unwrap(),
        );
        let mut foreign_filter = ptr::null_mut();
        assert_eq!(
            unsafe {
                filter_open(
                    &foreign,
                    &foreign_model,
                    &mut foreign_filter,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                filter_validate_model(filter, foreign_filter, &fixture.token, &mut diagnostics)
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "generated_token_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { filter_close(&mut foreign_filter) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
        assert_eq!(fixture.state.query_count(), 0);
    }

    #[test]
    fn iid_filters_copy_spelling_and_empty_lists_preserve_matches_none() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut source = open(&fixture);
        let mut exact = ptr::null_mut();
        let mut empty = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        {
            let iid = TypeBridgeIid::new("0xAb10").unwrap();
            // SAFETY: source, inline IID and independent outputs remain live.
            assert_eq!(
                unsafe { filter_iid(source, &iid, &mut exact, &mut diagnostics) },
                TypeBridgeStatus::Ok
            );
        }
        // SAFETY: zero count ignores even a dangling pointer table.
        assert_eq!(
            unsafe { filter_iids(exact, ptr::dangling(), 0, &mut empty, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { filter_close(&mut source) }, TypeBridgeStatus::Ok);
        assert_eq!(
            unsafe { &*exact }.value.exact_iids().unwrap()[0].as_str(),
            "0xAb10"
        );
        assert!(!unsafe { &*exact }.value.matches_none());
        assert!(unsafe { &*empty }.value.matches_none());
        assert_eq!(unsafe { filter_close(&mut exact) }, TypeBridgeStatus::Ok);
        assert!(unsafe { &*empty }.value.matches_none());
        assert_eq!(unsafe { filter_close(&mut empty) }, TypeBridgeStatus::Ok);
        assert_eq!(fixture.state.query_count(), 0);
    }

    #[test]
    fn iid_owner_aliases_null_elements_and_allocation_failures_preserve_source() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut source = open(&fixture);
        let iid = TypeBridgeIid::new("0x10").unwrap();
        let snapshot = iid.as_str().to_owned();
        let alias = (&iid as *const TypeBridgeIid).cast_mut().cast();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the forged overlapping output is refused before writes.
        assert_eq!(
            unsafe { filter_iid(source, &iid, alias, &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(iid.as_str(), snapshot);
        let mut result = ptr::dangling_mut();
        let table = [ptr::null()];
        assert_eq!(
            unsafe { filter_iids(source, table.as_ptr(), 1, &mut result, &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(result.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_manager_filter_input_invalid");
        close_diagnostics(&mut diagnostics);
        {
            let _failure = inject_failure(AllocationSite::ManagerFilterHandle, 0);
            assert_eq!(
                unsafe { filter_iid(source, &iid, &mut result, &mut diagnostics) },
                TypeBridgeStatus::ResourceLimit
            );
        }
        assert!(result.is_null());
        close_diagnostics(&mut diagnostics);
        assert!(unsafe { &*source }.value.is_empty());
        assert_eq!(unsafe { filter_close(&mut source) }, TypeBridgeStatus::Ok);
    }

    #[test]
    fn field_filter_uses_nominal_tokens_and_shared_comparison_rules() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut source = open(&fixture);
        let (field, values) = fixture
            .create
            .value
            .fields()
            .iter()
            .find(|(_, values)| !values.is_empty())
            .unwrap();
        let identity = ProjectedTokenIdentity::Field {
            owner: field.owner().clone(),
            field: field.clone(),
        };
        let mut token = fixture.token;
        token.kind = type_bridge_contract::projection::ProjectedTokenKind::Field.as_u32();
        token.ordinal = fixture
            .package
            .state
            ._projection
            .projected_token_ordinal(&identity)
            .unwrap();
        let value = TypeBridgeProjectedValue::from_owned(
            Arc::clone(&fixture.package.state),
            values[0].clone(),
        );
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: nominal field/value owners and output slots are live and disjoint.
        assert_eq!(
            unsafe { filter_compare(source, &token, 1, &value, &mut result, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*result }.value.len(), 1);
        assert!(unsafe { &*source }.value.is_empty());
        assert_eq!(unsafe { filter_close(&mut result) }, TypeBridgeStatus::Ok);
        assert_eq!(
            unsafe { filter_compare(source, &token, 7, &value, &mut result, &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(result.is_null());
        close_diagnostics(&mut diagnostics);
        assert_eq!(unsafe { filter_close(&mut source) }, TypeBridgeStatus::Ok);
    }
}
