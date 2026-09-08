//! Generated-only streaming construction for bounded projected create values.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::id::{RoleId, TypeId};
use type_bridge_contract::projection::ProjectedTokenKind;
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::{
    ProjectedAttributeValue, ProjectedCreate, ProjectedCreateBudget, ProjectedReference,
};

use crate::abi::{SchemaPackageState, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted, try_box, try_reserve};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_PROJECTED_TOKEN, GENERATED_INPUT_SCHEMA_PACKAGE,
    direct_output_preflight,
};
use crate::projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedReference, check_field_id_ranges,
    check_projected_attribute_value_ranges, check_projected_reference_ranges, check_role_id_ranges,
    check_type_id_ranges,
};
use crate::projected_token::{
    TypeBridgeProjectedTokenV1, resolve_field_token, resolve_model_token, resolve_role_token,
};
use crate::projected_value::{
    TypeBridgeProjectedValue, invalid_brand_diagnostic, same_package_brand,
};

const PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX: usize = 256;

/// Opaque generated-only streaming projected-create builder.
pub struct TypeBridgeProjectedCreateBuilder {
    package: Arc<SchemaPackageState>,
    model: TypeId,
    fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
    roles: Vec<(RoleId, Vec<ProjectedReference>)>,
    budget: ProjectedCreateBudget,
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C projected-create builder code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C projected-create builder message is valid")
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn invalid_lane() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_create_builder_handle_lane_invalid",
        "The projected create builder member requires exactly one matching handle lane",
    )
}

fn invalid_handle_array() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_create_builder_handle_array_invalid",
        "The projected create builder handle array is invalid",
    )
}

fn owner_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_create_builder_member_owner_mismatch",
        "The projected create builder member token belongs to a different model",
    )
}

fn invalid_member_token() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_create_builder_member_token_invalid",
        "The projected create builder member token is not a field or role",
    )
}

fn preflight_builder_borrowed_ranges(
    builder: &TypeBridgeProjectedCreateBuilder,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_package_borrowed_ranges(&builder.package)?;
    check_type_id_ranges(&builder.model, preflight)?;
    preflight.check_bytes(
        builder.fields.as_ptr().cast(),
        builder
            .fields
            .len()
            .checked_mul(size_of::<(OwnsFactId, Vec<ProjectedAttributeValue>)>())
            .ok_or(TypeBridgeStatus::ResourceLimit)?,
    )?;
    for (field, values) in &builder.fields {
        check_field_id_ranges(field, preflight)?;
        preflight.check_bytes(
            values.as_ptr().cast(),
            values
                .len()
                .checked_mul(size_of::<ProjectedAttributeValue>())
                .ok_or(TypeBridgeStatus::ResourceLimit)?,
        )?;
        for value in values {
            check_projected_attribute_value_ranges(value, preflight)?;
        }
    }
    preflight.check_bytes(
        builder.roles.as_ptr().cast(),
        builder
            .roles
            .len()
            .checked_mul(size_of::<(RoleId, Vec<ProjectedReference>)>())
            .ok_or(TypeBridgeStatus::ResourceLimit)?,
    )?;
    for (role, references) in &builder.roles {
        check_role_id_ranges(role, preflight)?;
        preflight.check_bytes(
            references.as_ptr().cast(),
            references
                .len()
                .checked_mul(size_of::<ProjectedReference>())
                .ok_or(TypeBridgeStatus::ResourceLimit)?,
        )?;
        for reference in references {
            check_projected_reference_ranges(reference, preflight)?;
        }
    }
    Ok(())
}

unsafe fn preflight_value_array(
    preflight: &DirectOutputPreflight,
    values: *const *const TypeBridgeProjectedValue,
    count: usize,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: the caller retains the complete bounded pointer array for this call.
    unsafe { preflight.check_pointer_array(values, count) }?;
    let mut total_members = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 0..count {
        // SAFETY: the pointer-array storage was checked above.
        let value = unsafe { values.add(index).read_unaligned() };
        if !value.is_null() {
            // SAFETY: each complete pointee object was checked above.
            let measure = unsafe { &*value }.value().resource_measure();
            total_members = total_members
                .checked_add(measure.members())
                .filter(|members| *members <= type_bridge_orm::MAX_PROJECTED_MODEL_MEMBERS)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
            total_bytes = total_bytes
                .checked_add(measure.bytes())
                .filter(|bytes| *bytes <= type_bridge_orm::MAX_PROJECTED_MODEL_BYTES)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
        }
    }
    for index in 0..count {
        // SAFETY: the pointer-array storage was checked above.
        let value = unsafe { values.add(index).read_unaligned() };
        if !value.is_null() {
            // SAFETY: each complete pointee object was checked above.
            unsafe { &*value }.check_borrowed_ranges(preflight)?;
        }
    }
    Ok(())
}

unsafe fn preflight_reference_array(
    preflight: &DirectOutputPreflight,
    references: *const *const TypeBridgeProjectedReference,
    count: usize,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: the caller retains the complete bounded pointer array for this call.
    unsafe { preflight.check_pointer_array(references, count) }?;
    let mut total_members = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 0..count {
        // SAFETY: the pointer-array storage was checked above.
        let reference = unsafe { references.add(index).read_unaligned() };
        if !reference.is_null() {
            // SAFETY: each complete pointee object was checked above.
            let measure = unsafe { &*reference }.value.resource_measure();
            total_members = total_members
                .checked_add(measure.members())
                .filter(|members| *members <= type_bridge_orm::MAX_PROJECTED_MODEL_MEMBERS)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
            total_bytes = total_bytes
                .checked_add(measure.bytes())
                .filter(|bytes| *bytes <= type_bridge_orm::MAX_PROJECTED_MODEL_BYTES)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
        }
    }
    for index in 0..count {
        // SAFETY: the pointer-array storage was checked above.
        let reference = unsafe { references.add(index).read_unaligned() };
        if !reference.is_null() {
            // SAFETY: each complete pointee object was checked above.
            unsafe { &*reference }.check_borrowed_ranges(preflight)?;
        }
    }
    Ok(())
}

/// Open one empty streaming builder for an exact generated create model.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_builder_open_v1(
    package: *const TypeBridgeSchemaPackage,
    model: *const TypeBridgeProjectedTokenV1,
    out_builder: *mut *mut TypeBridgeProjectedCreateBuilder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[
        (
            out_builder.cast(),
            size_of::<*mut TypeBridgeProjectedCreateBuilder>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for (kind, pointer) in [
        (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
        (GENERATED_INPUT_PROJECTED_TOKEN, model.cast()),
    ] {
        if let Err(status) = preflight.check_object_kind(kind, pointer) {
            return status;
        }
    }
    if !package.is_null() {
        // SAFETY: the complete package object was proven disjoint above.
        let package = unsafe { &*package };
        if let Err(status) = preflight.check_package_borrowed_ranges(package.state()) {
            return status;
        }
    }
    // SAFETY: both required output slots are non-null and pairwise disjoint.
    if let Err(status) = unsafe { initialize_execution_outputs(out_builder, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: preflight proved the complete immutable package object readable.
        let package = unsafe { &*package };
        // SAFETY: generated token storage remains readable for this call.
        let model = match unsafe { resolve_model_token(package.state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let budget =
            match ProjectedCreateBudget::try_new(&package.state().installed_projection, &model) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let builder = TypeBridgeProjectedCreateBuilder {
            package: Arc::clone(package.state()),
            model,
            fields: Vec::new(),
            roles: Vec::new(),
            budget,
        };
        let builder = match try_box(AllocationSite::ProjectedCreateBuilderHandle, builder) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: output initialization proved the slot writable and disjoint.
        unsafe { out_builder.write_unaligned(Box::into_raw(builder)) };
        TypeBridgeStatus::Ok
    })
}

/// Copy one nonempty bounded field-value or role-reference chunk into a builder.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_builder_add_v1(
    builder: *mut TypeBridgeProjectedCreateBuilder,
    member: *const TypeBridgeProjectedTokenV1,
    values: *const *const TypeBridgeProjectedValue,
    value_count: usize,
    references: *const *const TypeBridgeProjectedReference,
    reference_count: usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if value_count > PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX
        || reference_count > PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX
    {
        return TypeBridgeStatus::ResourceLimit;
    }
    let preflight = match direct_output_preflight(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for (kind, pointer) in [
        (GENERATED_INPUT_PROJECTED_TOKEN, member.cast()),
        (0, builder.cast()),
    ] {
        let result = if kind == 0 {
            preflight.check_bytes(pointer, size_of::<TypeBridgeProjectedCreateBuilder>())
        } else {
            preflight.check_object_kind(kind, pointer)
        };
        if let Err(status) = result {
            return status;
        }
    }
    if !builder.is_null() {
        // SAFETY: the complete builder object was proven disjoint above.
        let builder_ref = unsafe { &*builder };
        if let Err(status) = preflight_builder_borrowed_ranges(builder_ref, &preflight) {
            return status;
        }
    }
    if value_count != 0
        && let Err(status) = unsafe { preflight_value_array(&preflight, values, value_count) }
    {
        return status;
    }
    if reference_count != 0
        && let Err(status) =
            unsafe { preflight_reference_array(&preflight, references, reference_count) }
    {
        return status;
    }
    // SAFETY: strict preflight proved this one required slot writable.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        if builder.is_null() || member.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: all complete builder and token bytes were preflighted.
        let builder = unsafe { &mut *builder };
        // SAFETY: unaligned snapshot does not impose a generated storage alignment rule.
        let token_kind = ProjectedTokenKind::from_u32(unsafe { member.read_unaligned() }.kind);
        match token_kind {
            Some(ProjectedTokenKind::Field) => {
                if value_count == 0
                    || values.is_null()
                    || reference_count != 0
                    || !references.is_null()
                {
                    return return_execution_error(invalid_lane(), out_diagnostics);
                }
                // SAFETY: token layout and package brand are validated by the resolver.
                let (owner, field) = match unsafe { resolve_field_token(&builder.package, member) }
                {
                    Ok(value) => value,
                    Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
                };
                if owner != builder.model {
                    return return_execution_error(owner_mismatch(), out_diagnostics);
                }
                let existing = builder.fields.iter().position(|(id, _)| id == &field);
                let start_index = existing.map_or(0, |index| builder.fields[index].1.len());
                for index in 0..value_count {
                    // SAFETY: the complete bounded pointer array was preflighted.
                    let handle = unsafe { values.add(index).read_unaligned() };
                    if handle.is_null() {
                        return return_execution_error(invalid_handle_array(), out_diagnostics);
                    }
                    // SAFETY: each complete immutable handle was preflighted.
                    let handle = unsafe { &*handle };
                    if !same_package_brand(&builder.package, handle.package()) {
                        return return_execution_error(invalid_brand_diagnostic(), out_diagnostics);
                    }
                    if let Err(diagnostic) = handle
                        .value()
                        .validate_for(&builder.package.installed_projection)
                    {
                        return return_execution_error(diagnostic, out_diagnostics);
                    }
                }
                let mut copied = Vec::new();
                if try_reserve(
                    &mut copied,
                    value_count,
                    AllocationSite::ProjectedCreateBuilderChunk,
                )
                .is_err()
                {
                    return return_execution_error(allocation_exhausted(), out_diagnostics);
                }
                if let Some(index) = existing {
                    if try_reserve(
                        &mut builder.fields[index].1,
                        value_count,
                        AllocationSite::ProjectedCreateBuilderChunk,
                    )
                    .is_err()
                    {
                        return return_execution_error(allocation_exhausted(), out_diagnostics);
                    }
                } else if try_reserve(
                    &mut builder.fields,
                    1,
                    AllocationSite::ProjectedCreateBuilderChunk,
                )
                .is_err()
                {
                    return return_execution_error(allocation_exhausted(), out_diagnostics);
                }
                let field_values = (0..value_count).map(|index| {
                    // SAFETY: validation above rejected every null handle.
                    unsafe { (*values.add(index).read_unaligned()).value() }
                });
                if let Err(diagnostic) = builder.budget.try_add_field_values(
                    &builder.package.installed_projection,
                    &field,
                    start_index,
                    field_values,
                ) {
                    return return_execution_error(diagnostic, out_diagnostics);
                }
                for index in 0..value_count {
                    // SAFETY: validation above rejected every null handle.
                    copied.push(unsafe { (*values.add(index).read_unaligned()).value().clone() });
                }
                if let Some(index) = existing {
                    builder.fields[index].1.append(&mut copied);
                } else {
                    builder.fields.push((field, copied));
                }
                TypeBridgeStatus::Ok
            }
            Some(ProjectedTokenKind::Role) => {
                if reference_count == 0
                    || references.is_null()
                    || value_count != 0
                    || !values.is_null()
                {
                    return return_execution_error(invalid_lane(), out_diagnostics);
                }
                // SAFETY: token layout and package brand are validated by the resolver.
                let (owner, role) = match unsafe { resolve_role_token(&builder.package, member) } {
                    Ok(value) => value,
                    Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
                };
                if owner != builder.model {
                    return return_execution_error(owner_mismatch(), out_diagnostics);
                }
                let existing = builder.roles.iter().position(|(id, _)| id == &role);
                let start_index = existing.map_or(0, |index| builder.roles[index].1.len());
                for index in 0..reference_count {
                    // SAFETY: the complete bounded pointer array was preflighted.
                    let handle = unsafe { references.add(index).read_unaligned() };
                    if handle.is_null() {
                        return return_execution_error(invalid_handle_array(), out_diagnostics);
                    }
                    // SAFETY: each complete immutable handle was preflighted.
                    let handle = unsafe { &*handle };
                    if !same_package_brand(&builder.package, &handle.package) {
                        return return_execution_error(invalid_brand_diagnostic(), out_diagnostics);
                    }
                    if let Err(diagnostic) = handle
                        .value
                        .validate_for(&builder.package.installed_projection)
                    {
                        return return_execution_error(diagnostic, out_diagnostics);
                    }
                }
                let mut copied = Vec::new();
                if try_reserve(
                    &mut copied,
                    reference_count,
                    AllocationSite::ProjectedCreateBuilderChunk,
                )
                .is_err()
                {
                    return return_execution_error(allocation_exhausted(), out_diagnostics);
                }
                if let Some(index) = existing {
                    if try_reserve(
                        &mut builder.roles[index].1,
                        reference_count,
                        AllocationSite::ProjectedCreateBuilderChunk,
                    )
                    .is_err()
                    {
                        return return_execution_error(allocation_exhausted(), out_diagnostics);
                    }
                } else if try_reserve(
                    &mut builder.roles,
                    1,
                    AllocationSite::ProjectedCreateBuilderChunk,
                )
                .is_err()
                {
                    return return_execution_error(allocation_exhausted(), out_diagnostics);
                }
                let role_references = (0..reference_count).map(|index| {
                    // SAFETY: validation above rejected every null handle.
                    unsafe { &(*references.add(index).read_unaligned()).value }
                });
                if let Err(diagnostic) = builder.budget.try_add_role_references(
                    &builder.package.installed_projection,
                    &role,
                    start_index,
                    role_references,
                ) {
                    return return_execution_error(diagnostic, out_diagnostics);
                }
                for index in 0..reference_count {
                    // SAFETY: validation above rejected every null handle.
                    copied.push(unsafe { (*references.add(index).read_unaligned()).value.clone() });
                }
                if let Some(index) = existing {
                    builder.roles[index].1.append(&mut copied);
                } else {
                    builder.roles.push((role, copied));
                }
                TypeBridgeStatus::Ok
            }
            Some(ProjectedTokenKind::Model) | Some(_) | None => {
                return_execution_error(invalid_member_token(), out_diagnostics)
            }
        }
    })
}

/// Finish one builder, consuming it on every post-preflight outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_builder_finish(
    builder: *mut *mut TypeBridgeProjectedCreateBuilder,
    out_create: *mut *mut TypeBridgeProjectedCreate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            out_create.cast(),
            size_of::<*mut TypeBridgeProjectedCreate>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = outputs.check_bytes(
        builder.cast(),
        size_of::<*mut TypeBridgeProjectedCreateBuilder>(),
    ) {
        return status;
    }
    let builder_pointer = unsafe { builder.read_unaligned() };
    if !builder_pointer.is_null() {
        if let Err(status) = outputs.check_bytes(
            builder_pointer.cast(),
            size_of::<TypeBridgeProjectedCreateBuilder>(),
        ) {
            return status;
        }
        // SAFETY: the complete builder object is disjoint from both outputs.
        let builder_ref = unsafe { &*builder_pointer };
        if let Err(status) = preflight_builder_borrowed_ranges(builder_ref, &outputs) {
            return status;
        }
        let owner_slot = match direct_output_preflight(&[(
            builder.cast(),
            size_of::<*mut TypeBridgeProjectedCreateBuilder>(),
        )]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        if let Err(status) = owner_slot.check_bytes(
            builder_pointer.cast(),
            size_of::<TypeBridgeProjectedCreateBuilder>(),
        ) {
            return status;
        }
        if let Err(status) = preflight_builder_borrowed_ranges(builder_ref, &owner_slot) {
            return status;
        }
    }
    // SAFETY: both result slots are writable, pairwise disjoint, and disjoint from ownership.
    if let Err(status) = unsafe { initialize_execution_outputs(out_create, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if builder_pointer.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: preflight proved the ownership slot writable and the pointer uniquely owned.
        unsafe { builder.write_unaligned(ptr::null_mut()) };
        // SAFETY: the caller transfers the one live builder allocation on this terminal path.
        let builder = unsafe { Box::from_raw(builder_pointer) };
        let output = match ReservedBox::try_new(AllocationSite::ProjectedCreateHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let TypeBridgeProjectedCreateBuilder {
            package,
            model,
            fields,
            roles,
            budget: _,
        } = *builder;
        let create =
            match ProjectedCreate::try_new(&package.installed_projection, model, fields, roles) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let create = output.initialize(TypeBridgeProjectedCreate {
            package,
            value: create,
        });
        // SAFETY: result output was initialized and remains writable.
        unsafe { out_create.write_unaligned(Box::into_raw(create)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one unfinished builder and clear its ownership slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_builder_close(
    builder: *mut *mut TypeBridgeProjectedCreateBuilder,
) -> TypeBridgeStatus {
    if builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    let slot = match direct_output_preflight(&[(
        builder.cast(),
        size_of::<*mut TypeBridgeProjectedCreateBuilder>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    let builder_pointer = unsafe { builder.read_unaligned() };
    if !builder_pointer.is_null() {
        if let Err(status) = slot.check_bytes(
            builder_pointer.cast(),
            size_of::<TypeBridgeProjectedCreateBuilder>(),
        ) {
            return status;
        }
        // SAFETY: the complete builder object was proven disjoint from its ownership slot.
        let builder_ref = unsafe { &*builder_pointer };
        if let Err(status) = preflight_builder_borrowed_ranges(builder_ref, &slot) {
            return status;
        }
    }
    guarded(|| {
        // SAFETY: the ownership slot is non-null and writable.
        unsafe { builder.write_unaligned(ptr::null_mut()) };
        if !builder_pointer.is_null() {
            // SAFETY: the caller transfers exactly one live builder allocation.
            drop(unsafe { Box::from_raw(builder_pointer) });
        }
        TypeBridgeStatus::Ok
    })
}

#[cfg(test)]
mod tests {
    use std::mem::{MaybeUninit, size_of};
    use std::ptr;
    use std::sync::Arc;

    use type_bridge_contract::id::{AttributeId, TypeKind};
    use type_bridge_contract::projection::{
        ProjectedTokenIdentity, TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
    };
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};

    use super::*;
    use crate::abi::TypeBridgeByteView;
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{model_token, package};
    use crate::execution_diagnostic::{
        TypeBridgeExecutionDiagnosticViewV1, type_bridge_execution_diagnostics_close,
        type_bridge_execution_diagnostics_get_v1,
    };
    use crate::projected_value::type_bridge_projected_value_text;

    fn field_token(
        package: &TypeBridgeSchemaPackage,
        owner: &TypeId,
        attribute: &str,
    ) -> TypeBridgeProjectedTokenV1 {
        let field = OwnsFactId::new(
            owner.clone(),
            AttributeId::new(attribute).expect("test attribute label is canonical"),
        )
        .expect("test field identity is valid");
        let identity = ProjectedTokenIdentity::Field {
            owner: owner.clone(),
            field,
        };
        let ordinal = package
            .state
            ._projection
            .projected_token_ordinal(&identity)
            .expect("test field is projected");
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: ProjectedTokenKind::Field.as_u32(),
            ordinal,
            projection_digest: package
                .state
                ._projection
                .projection_fingerprint()
                .as_fingerprint()
                .digest()
                .bytes(),
            reserved: [0; 4],
        }
    }

    fn identifier_value(package: &TypeBridgeSchemaPackage) -> Box<TypeBridgeProjectedValue> {
        let attribute = TypeId::new(TypeKind::Attribute, "identifier")
            .expect("test attribute identity is valid");
        let value = ProjectedAttributeValue::try_new(
            &package.state.installed_projection,
            attribute,
            CanonicalValue::String(CanonicalString::new("ada").expect("test string is canonical")),
        )
        .expect("test projected value is valid");
        Box::new(TypeBridgeProjectedValue::from_owned(
            Arc::clone(&package.state),
            value,
        ))
    }

    fn large_identifier_value(
        package: &TypeBridgeSchemaPackage,
        length: usize,
    ) -> Box<TypeBridgeProjectedValue> {
        let attribute = TypeId::new(TypeKind::Attribute, "identifier")
            .expect("test attribute identity is valid");
        let lexical = "x".repeat(length);
        let value = ProjectedAttributeValue::try_new(
            &package.state.installed_projection,
            attribute,
            CanonicalValue::String(
                CanonicalString::new(lexical.clone()).expect("test string is canonical"),
            ),
        )
        .expect("test projected string is valid");
        Box::new(TypeBridgeProjectedValue::from_owned(
            Arc::clone(&package.state),
            value,
        ))
    }

    unsafe fn diagnostic_code(diagnostics: *const TypeBridgeExecutionDiagnostics) -> String {
        let mut view = MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            // SAFETY: the retained diagnostic and uninitialized writable result slot are live.
            unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, view.as_mut_ptr()) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: a successful getter initialized the complete view.
        let view = unsafe { view.assume_init() };
        // SAFETY: the view borrows stable bytes from the retained diagnostic handle.
        let bytes = unsafe { std::slice::from_raw_parts(view.code.data, view.code.length) };
        String::from_utf8(bytes.to_vec()).expect("diagnostic code is UTF-8")
    }

    unsafe fn close_diagnostics(diagnostics: &mut *mut TypeBridgeExecutionDiagnostics) {
        assert_eq!(
            // SAFETY: the slot uniquely owns the diagnostic handle.
            unsafe { type_bridge_execution_diagnostics_close(diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
    }

    unsafe fn open_builder(
        package: &TypeBridgeSchemaPackage,
        model: &TypeBridgeProjectedTokenV1,
    ) -> *mut TypeBridgeProjectedCreateBuilder {
        let mut builder = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: retained inputs and both writable output slots remain live.
            unsafe {
                type_bridge_projected_create_builder_open_v1(
                    package,
                    model,
                    &mut builder,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(!builder.is_null());
        assert!(diagnostics.is_null());
        builder
    }

    unsafe fn add_identifier(
        builder: *mut TypeBridgeProjectedCreateBuilder,
        field: &TypeBridgeProjectedTokenV1,
        value: &TypeBridgeProjectedValue,
        diagnostics: &mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus {
        let values = [value as *const TypeBridgeProjectedValue];
        // SAFETY: the builder, token, one-element array, pointee, and diagnostic slot remain live.
        unsafe {
            type_bridge_projected_create_builder_add_v1(
                builder,
                field,
                values.as_ptr(),
                values.len(),
                ptr::null::<*const TypeBridgeProjectedReference>(),
                0,
                diagnostics,
            )
        }
    }

    #[test]
    fn builder_handle_allocation_failure_is_structured_and_retryable() {
        let package = package("builderhandlealloc");
        let person = TypeId::new(TypeKind::Entity, "person").expect("test model is valid");
        let model = model_token(&package, person);
        let mut builder = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let _failure = inject_failure(AllocationSite::ProjectedCreateBuilderHandle, 0);
        assert_eq!(
            // SAFETY: retained inputs and both writable output slots remain live.
            unsafe {
                type_bridge_projected_create_builder_open_v1(
                    &package,
                    &model,
                    &mut builder,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(builder.is_null());
        assert!(!diagnostics.is_null());
        // SAFETY: the diagnostic remains live for this inspection and close.
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        // SAFETY: the slot uniquely owns the diagnostic handle.
        unsafe { close_diagnostics(&mut diagnostics) };

        // The one-shot failure was consumed; the same immutable inputs are reusable.
        // SAFETY: retained inputs satisfy the builder-open contract.
        builder = unsafe { open_builder(&package, &model) };
        // SAFETY: the slot uniquely owns this unfinished builder.
        assert_eq!(
            unsafe { type_bridge_projected_create_builder_close(&mut builder) },
            TypeBridgeStatus::Ok,
        );
        assert!(builder.is_null());
    }

    #[test]
    fn builder_chunk_allocation_failure_is_atomic_reusable_and_closable() {
        let package = package("builderchunkalloc");
        let person = TypeId::new(TypeKind::Entity, "person").expect("test model is valid");
        let model = model_token(&package, person.clone());
        let field = field_token(&package, &person, "identifier");
        let value = identifier_value(&package);
        // SAFETY: retained package/model satisfy the builder-open contract.
        let mut builder = unsafe { open_builder(&package, &model) };
        let mut diagnostics = ptr::null_mut();
        let _failure = inject_failure(AllocationSite::ProjectedCreateBuilderChunk, 0);
        assert_eq!(
            // SAFETY: all retained inputs and the diagnostic slot remain live.
            unsafe { add_identifier(builder, &field, &value, &mut diagnostics) },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(!diagnostics.is_null());
        // SAFETY: the diagnostic remains live for this inspection and close.
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        // SAFETY: the slot uniquely owns the diagnostic handle.
        unsafe { close_diagnostics(&mut diagnostics) };
        // SAFETY: failure happened before budget/logical mutation.
        assert!(unsafe { &*builder }.fields.is_empty());
        assert!(unsafe { &*builder }.roles.is_empty());

        assert_eq!(
            // SAFETY: the same live builder and immutable inputs are reusable.
            unsafe { add_identifier(builder, &field, &value, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        assert_eq!(unsafe { &*builder }.fields[0].1.len(), 1);
        // SAFETY: the slot uniquely owns the still-live builder after a successful retry.
        assert_eq!(
            unsafe { type_bridge_projected_create_builder_close(&mut builder) },
            TypeBridgeStatus::Ok,
        );
        assert!(builder.is_null());
    }

    #[test]
    fn builder_rejects_a_foreign_value_with_the_common_package_diagnostic() {
        let local = package("builderlocalbrand");
        let foreign = package("builderforeignbrand");
        let person = TypeId::new(TypeKind::Entity, "person").expect("test model is valid");
        let model = model_token(&local, person.clone());
        let field = field_token(&local, &person, "identifier");
        let foreign_value = identifier_value(&foreign);
        let local_value = identifier_value(&local);
        // SAFETY: retained package/model satisfy the builder-open contract.
        let mut builder = unsafe { open_builder(&local, &model) };
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: the builder, token, foreign value, and output remain live.
            unsafe { add_identifier(builder, &field, &foreign_value, &mut diagnostics) },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(!diagnostics.is_null());
        // SAFETY: the diagnostic remains live for this inspection and close.
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "generated_token_package_mismatch",
        );
        // SAFETY: the slot uniquely owns the diagnostic handle.
        unsafe { close_diagnostics(&mut diagnostics) };
        // Package fencing precedes every logical builder mutation.
        assert!(unsafe { &*builder }.fields.is_empty());

        assert_eq!(
            // SAFETY: the same builder remains reusable with local immutable input.
            unsafe { add_identifier(builder, &field, &local_value, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        // SAFETY: the slot uniquely owns the still-live builder.
        assert_eq!(
            unsafe { type_bridge_projected_create_builder_close(&mut builder) },
            TypeBridgeStatus::Ok,
        );
        assert!(builder.is_null());
    }

    #[test]
    fn builder_finish_handle_reservation_failure_consumes_and_cleans_builder() {
        let package = package("builderfinishalloc");
        let person = TypeId::new(TypeKind::Entity, "person").expect("test model is valid");
        let model = model_token(&package, person.clone());
        let field = field_token(&package, &person, "identifier");
        let value = identifier_value(&package);
        // SAFETY: retained package/model satisfy the builder-open contract.
        let mut builder = unsafe { open_builder(&package, &model) };
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: all retained inputs and the diagnostic slot remain live.
            unsafe { add_identifier(builder, &field, &value, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());

        let _failure = inject_failure(AllocationSite::ProjectedCreateHandle, 0);
        let mut create = ptr::null_mut();
        assert_eq!(
            // SAFETY: ownership and both result slots are live and pairwise disjoint.
            unsafe {
                type_bridge_projected_create_builder_finish(
                    &mut builder,
                    &mut create,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(
            builder.is_null(),
            "terminal finish must consume the builder"
        );
        assert!(create.is_null());
        assert!(!diagnostics.is_null());
        // SAFETY: the diagnostic remains live for this inspection and close.
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        // SAFETY: the slot uniquely owns the diagnostic handle.
        unsafe { close_diagnostics(&mut diagnostics) };
    }

    #[test]
    fn builder_add_rejects_repeated_large_values_before_deep_alias_precedence() {
        let package = package("buildermeasurelimit");
        let person = TypeId::new(TypeKind::Entity, "person").expect("test model is valid");
        let model = model_token(&package, person.clone());
        let field = field_token(&package, &person, "identifier");
        let value = large_identifier_value(&package, 70_000);
        // SAFETY: retained package/model satisfy the builder-open contract.
        let mut builder = unsafe { open_builder(&package, &model) };
        let values = vec![&*value as *const TypeBridgeProjectedValue; 256];
        let mut borrowed = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        // SAFETY: the retained value and writable view remain live.
        assert_eq!(
            unsafe { type_bridge_projected_value_text(&*value, &mut borrowed) },
            TypeBridgeStatus::Ok,
        );
        let before = unsafe { std::slice::from_raw_parts(borrowed.data, borrowed.length) }.to_vec();
        let diagnostics = borrowed
            .data
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            // SAFETY: the repeated array is complete; hostile output lies in exposed text.
            unsafe {
                type_bridge_projected_create_builder_add_v1(
                    builder,
                    &field,
                    values.as_ptr(),
                    values.len(),
                    ptr::null::<*const TypeBridgeProjectedReference>(),
                    0,
                    diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(borrowed.data, borrowed.length) },
            before,
        );
        assert!(unsafe { &*builder }.fields.is_empty());
        // SAFETY: the failed add did not consume or poison the builder.
        assert_eq!(
            unsafe { type_bridge_projected_create_builder_close(&mut builder) },
            TypeBridgeStatus::Ok,
        );
        assert!(builder.is_null());
    }
}
