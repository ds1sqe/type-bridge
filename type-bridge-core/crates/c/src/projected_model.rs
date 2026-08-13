use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES};
use type_bridge_contract::projection::{ProjectedContainer, ProjectedTokenIdentity};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::{
    MAX_PROJECTED_MODEL_BYTES, MAX_PROJECTED_MODEL_MEMBERS, ProjectedAttributeValue,
    ProjectedCreate, ProjectedReference, ProjectedRolePlayer, ProjectedThing,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus,
    borrowed_view, close_box, guarded, initialize_view,
};
use crate::allocation::{AllocationSite, allocation_exhausted, try_box};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_PROJECTED_CREATE, GENERATED_INPUT_PROJECTED_REFERENCE,
    GENERATED_INPUT_PROJECTED_THING, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_SCHEMA_PACKAGE, direct_output_preflight,
};
use crate::projected_token::{
    TypeBridgeProjectedTokenV1, resolve_field_token, resolve_model_token, resolve_role_token,
};
use crate::projected_value::{
    TypeBridgeProjectedValue, invalid_brand_diagnostic, invalid_shape_diagnostic,
    same_package_brand, snapshot_handle_array,
};

const PROJECTED_MODEL_INPUT_VERSION: u32 = 1;

/// Version-1 projected field input containing immutable scalar handles.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeProjectedFieldInputV1 {
    /// Exact size of this input.
    pub struct_size: u32,
    /// Projected-model input layout version.
    pub version: u32,
    /// Generated field token.
    pub field: *const TypeBridgeProjectedTokenV1,
    /// Readable array of immutable scalar handles.
    pub values: *const *const TypeBridgeProjectedValue,
    /// Number of scalar handles.
    pub value_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 projected role input containing immutable reference handles.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeProjectedRoleInputV1 {
    /// Exact size of this input.
    pub struct_size: u32,
    /// Projected-model input layout version.
    pub version: u32,
    /// Generated owner-branded role token.
    pub role: *const TypeBridgeProjectedTokenV1,
    /// Readable array of immutable reference handles.
    pub references: *const *const TypeBridgeProjectedReference,
    /// Number of reference handles.
    pub reference_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 descriptor for an immutable projected reference.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeProjectedReferenceDescriptorV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Projected-model input layout version.
    pub version: u32,
    /// Generated model token.
    pub model: *const TypeBridgeProjectedTokenV1,
    /// Optional canonical IID. An empty view means no IID.
    pub iid: TypeBridgeByteView,
    /// Readable array of projected key field inputs.
    pub keys: *const TypeBridgeProjectedFieldInputV1,
    /// Number of key field inputs.
    pub key_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 descriptor for an immutable projected create value.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeProjectedCreateDescriptorV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Projected-model input layout version.
    pub version: u32,
    /// Generated model token.
    pub model: *const TypeBridgeProjectedTokenV1,
    /// Readable array of projected field inputs.
    pub fields: *const TypeBridgeProjectedFieldInputV1,
    /// Number of field inputs.
    pub field_count: usize,
    /// Readable array of projected role inputs.
    pub roles: *const TypeBridgeProjectedRoleInputV1,
    /// Number of role inputs.
    pub role_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 descriptor for an immutable completely hydrated projected thing.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeProjectedThingDescriptorV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Projected-model input layout version.
    pub version: u32,
    /// Generated model token.
    pub model: *const TypeBridgeProjectedTokenV1,
    /// Mandatory canonical provider IID.
    pub iid: TypeBridgeByteView,
    /// Readable array of complete projected field inputs.
    pub fields: *const TypeBridgeProjectedFieldInputV1,
    /// Number of field inputs.
    pub field_count: usize,
    /// Readable array of complete projected role-player inputs.
    pub roles: *const TypeBridgeProjectedRoleInputV1,
    /// Number of role inputs.
    pub role_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Opaque immutable package-branded projected reference.
pub struct TypeBridgeProjectedReference {
    pub(crate) package: Arc<SchemaPackageState>,
    pub(crate) value: ProjectedReference,
}

/// Opaque immutable package-branded projected create value.
pub struct TypeBridgeProjectedCreate {
    pub(crate) package: Arc<SchemaPackageState>,
    pub(crate) value: ProjectedCreate,
}

/// Opaque immutable package-branded completely hydrated projected thing.
pub struct TypeBridgeProjectedThing {
    pub(crate) package: Arc<SchemaPackageState>,
    pub(crate) value: Arc<ProjectedThing>,
}

trait ProjectedHandleBorrowedRanges {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus>;
}

fn check_slice_range<T>(
    values: &[T],
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    let length = values
        .len()
        .checked_mul(size_of::<T>())
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    preflight.check_bytes(values.as_ptr().cast(), length)
}

fn check_text_range(
    value: &str,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(value.as_ptr().cast(), value.len())
}

pub(crate) fn check_type_id_ranges(
    value: &type_bridge_contract::id::TypeId,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    check_text_range(value.label().as_str(), preflight)
}

pub(crate) fn check_field_id_ranges(
    value: &OwnsFactId,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes((value as *const OwnsFactId).cast(), size_of::<OwnsFactId>())?;
    check_type_id_ranges(value.owner(), preflight)?;
    check_text_range(value.attribute().label().as_str(), preflight)
}

pub(crate) fn check_role_id_ranges(
    value: &type_bridge_contract::id::RoleId,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(
        (value as *const type_bridge_contract::id::RoleId).cast(),
        size_of::<type_bridge_contract::id::RoleId>(),
    )?;
    check_text_range(value.declaring_relation().as_str(), preflight)?;
    check_text_range(value.label().as_str(), preflight)
}

pub(crate) fn check_projected_attribute_value_ranges(
    value: &ProjectedAttributeValue,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(
        (value as *const ProjectedAttributeValue).cast(),
        size_of::<ProjectedAttributeValue>(),
    )?;
    check_type_id_ranges(value.attribute_type(), preflight)?;
    match value.value() {
        CanonicalValue::String(value) => check_text_range(value.as_str(), preflight),
        CanonicalValue::Decimal(value) => check_text_range(value.as_str(), preflight),
        CanonicalValue::Long(_)
        | CanonicalValue::Double(_)
        | CanonicalValue::Boolean(_)
        | CanonicalValue::Date(_)
        | CanonicalValue::DateTime(_)
        | CanonicalValue::DateTimeTz(_)
        | CanonicalValue::Duration(_) => Ok(()),
    }
}

pub(crate) fn check_projected_reference_ranges(
    reference: &ProjectedReference,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(
        (reference as *const ProjectedReference).cast(),
        size_of::<ProjectedReference>(),
    )?;
    check_type_id_ranges(reference.type_id(), preflight)?;
    if let Some(iid) = reference.iid() {
        check_text_range(iid, preflight)?;
    }
    for (field, value) in reference.keys() {
        check_field_id_ranges(field, preflight)?;
        check_projected_attribute_value_ranges(value, preflight)?;
    }
    Ok(())
}

fn check_projected_create_ranges(
    create: &ProjectedCreate,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    if create.resource_measure().members() > MAX_PROJECTED_MODEL_MEMBERS {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    check_type_id_ranges(create.type_id(), preflight)?;
    for (field, values) in create.fields() {
        check_field_id_ranges(field, preflight)?;
        preflight.check_bytes(
            (values as *const Vec<ProjectedAttributeValue>).cast(),
            size_of::<Vec<ProjectedAttributeValue>>(),
        )?;
        check_slice_range(values, preflight)?;
        for value in values {
            check_projected_attribute_value_ranges(value, preflight)?;
        }
    }
    for (role, references) in create.roles() {
        check_role_id_ranges(role, preflight)?;
        preflight.check_bytes(
            (references as *const Vec<ProjectedReference>).cast(),
            size_of::<Vec<ProjectedReference>>(),
        )?;
        check_slice_range(references, preflight)?;
        for reference in references {
            check_projected_reference_ranges(reference, preflight)?;
        }
    }
    Ok(())
}

pub(crate) fn check_projected_thing_ranges(
    thing: &ProjectedThing,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    if thing.resource_measure().members() > MAX_PROJECTED_MODEL_MEMBERS {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    check_type_id_ranges(thing.type_id(), preflight)?;
    check_text_range(thing.iid(), preflight)?;
    for (field, values) in thing.fields() {
        check_field_id_ranges(field, preflight)?;
        preflight.check_bytes(
            (values as *const Vec<ProjectedAttributeValue>).cast(),
            size_of::<Vec<ProjectedAttributeValue>>(),
        )?;
        check_slice_range(values, preflight)?;
        for value in values {
            check_projected_attribute_value_ranges(value, preflight)?;
        }
    }
    for (role, players) in thing.roles() {
        check_role_id_ranges(role, preflight)?;
        preflight.check_bytes(
            (players as *const Vec<ProjectedRolePlayer>).cast(),
            size_of::<Vec<ProjectedRolePlayer>>(),
        )?;
        check_slice_range(players, preflight)?;
        for player in players {
            check_projected_reference_ranges(player.reference(), preflight)?;
        }
    }
    Ok(())
}

impl ProjectedHandleBorrowedRanges for TypeBridgeProjectedCreate {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_projected_create_ranges(&self.value, preflight)
    }
}

impl TypeBridgeProjectedCreate {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        <Self as ProjectedHandleBorrowedRanges>::check_borrowed_ranges(self, preflight)
    }
}

impl ProjectedHandleBorrowedRanges for TypeBridgeProjectedReference {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_projected_reference_ranges(&self.value, preflight)
    }
}

impl TypeBridgeProjectedReference {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        <Self as ProjectedHandleBorrowedRanges>::check_borrowed_ranges(self, preflight)
    }
}

impl ProjectedHandleBorrowedRanges for TypeBridgeProjectedThing {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_projected_thing_ranges(&self.value, preflight)
    }
}

impl TypeBridgeProjectedThing {
    pub(crate) fn from_arc(package: Arc<SchemaPackageState>, value: Arc<ProjectedThing>) -> Self {
        Self { package, value }
    }

    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        <Self as ProjectedHandleBorrowedRanges>::check_borrowed_ranges(self, preflight)
    }
}

unsafe fn check_handle_borrowed_ranges<T: ProjectedHandleBorrowedRanges>(
    handle: *const T,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    if !handle.is_null() {
        // SAFETY: the complete opaque handle object was checked before this read.
        unsafe { &*handle }.check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

fn descriptor_invalid() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    invalid_shape_diagnostic(
        "c_projected_model_descriptor_invalid",
        "The projected model descriptor layout is invalid",
    )
}

fn collection_limit() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
    };
    SdkExecutionDiagnostic::resource_limit(
        SdkDiagnosticCode::new("c_projected_model_collection_limit_exceeded")
            .expect("static diagnostic code is canonical"),
        SdkDiagnosticMessage::new(
            "The projected model input exceeds its stable collection ceiling",
        )
        .expect("static diagnostic message is valid"),
    )
}

fn scalar_hydration_integrity(
    code_value: &'static str,
    message_value: &'static str,
) -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
    };
    SdkExecutionDiagnostic::integrity(
        SdkDiagnosticCode::new(code_value).expect("static diagnostic code is canonical"),
        SdkDiagnosticMessage::new(message_value).expect("static diagnostic message is valid"),
    )
}

fn index_out_of_bounds() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    invalid_shape_diagnostic(
        "c_projected_index_out_of_bounds",
        "The projected collection index is outside the available sequence",
    )
}

fn reference_key_missing() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    invalid_shape_diagnostic(
        "c_projected_reference_key_missing",
        "The projected reference does not carry the selected key value",
    )
}

fn thing_model_mismatch() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    invalid_shape_diagnostic(
        "c_projected_thing_model_mismatch",
        "The projected thing does not match the generated model token",
    )
}

fn reference_model_mismatch() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic {
    invalid_shape_diagnostic(
        "c_projected_reference_model_mismatch",
        "The projected reference does not match the generated model token",
    )
}

fn reference_role_player_mismatch() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic
{
    invalid_shape_diagnostic(
        "c_projected_reference_role_player_mismatch",
        "The projected reference model is not admitted by the generated role token",
    )
}

/// Return one Rust-owned structured resource diagnostic for generated collection exhaustion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_collection_limit_diagnostics(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: the non-null diagnostics output is caller-writable.
    unsafe { out_diagnostics.write(std::ptr::null_mut()) };
    guarded(|| return_execution_error(collection_limit(), out_diagnostics))
}

unsafe fn copied_optional_iid(
    input: TypeBridgeByteView,
) -> Result<Option<String>, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    if input.length == 0 {
        return Ok(None);
    }
    if input.length > MAX_CANONICAL_STRING_BYTES {
        return Err(collection_limit());
    }
    if input.data.is_null() {
        return Err(descriptor_invalid());
    }
    // SAFETY: caller promises the bounded view is readable for this call.
    let bytes = unsafe { std::slice::from_raw_parts(input.data, input.length) }.to_vec();
    String::from_utf8(bytes).map(Some).map_err(|_| {
        invalid_shape_diagnostic(
            "c_projected_model_iid_utf8_invalid",
            "The projected model IID is not valid UTF-8",
        )
    })
}

fn validate_layout(struct_size: u32, expected: usize, version: u32, reserved: [u64; 4]) -> bool {
    struct_size as usize == expected
        && version == PROJECTED_MODEL_INPUT_VERSION
        && reserved == [0; 4]
}

fn validate_array(
    pointer: *const (),
    length: usize,
) -> Result<(), type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    if length > MAX_CANONICAL_COLLECTION_LEN {
        return Err(collection_limit());
    }
    if length != 0 && pointer.is_null() {
        return Err(descriptor_invalid());
    }
    Ok(())
}

fn projected_value_bytes(
    value: &ProjectedAttributeValue,
) -> Result<usize, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    let canonical_bytes = to_canonical_json(value.value())
        .map_err(|_| {
            invalid_shape_diagnostic(
                "c_projected_value_measurement_failed",
                "The projected scalar could not be measured",
            )
        })?
        .len();
    value
        .attribute_type()
        .label()
        .as_str()
        .len()
        .checked_add(canonical_bytes)
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or_else(collection_limit)
}

fn projected_reference_bytes(
    reference: &ProjectedReference,
) -> Result<usize, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    let mut total = reference
        .type_id()
        .label()
        .as_str()
        .len()
        .checked_add(reference.iid().map_or(0, str::len))
        .ok_or_else(collection_limit)?;
    for (field, value) in reference.keys() {
        let value_bytes = projected_value_bytes(value)?;
        total = total
            .checked_add(field.owner().label().as_str().len())
            .and_then(|bytes| bytes.checked_add(field.attribute().label().as_str().len()))
            .and_then(|bytes| bytes.checked_add(value_bytes))
            .ok_or_else(collection_limit)?;
    }
    Ok(total)
}

unsafe fn collect_fields(
    package: &Arc<SchemaPackageState>,
    owner: &type_bridge_contract::id::TypeId,
    inputs: *const TypeBridgeProjectedFieldInputV1,
    count: usize,
    keys_only: bool,
) -> Result<
    Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
> {
    validate_array(inputs.cast(), count)?;
    let mut output = Vec::with_capacity(count);
    let mut total_values = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 0..count {
        // SAFETY: caller promises all bounded descriptor elements are readable.
        let input = unsafe { inputs.add(index).read_unaligned() };
        if !validate_layout(
            input.struct_size,
            size_of::<TypeBridgeProjectedFieldInputV1>(),
            input.version,
            input.reserved,
        ) {
            return Err(descriptor_invalid());
        }
        // SAFETY: generated token storage is readable for this call.
        let (field_owner, field) = unsafe { resolve_field_token(package, input.field) }?;
        if &field_owner != owner {
            return Err(invalid_shape_diagnostic(
                "c_projected_field_owner_mismatch",
                "The projected field token belongs to a different model",
            ));
        }
        // SAFETY: pointer-array validation and snapshotting are centralized.
        let handles = unsafe { snapshot_handle_array(input.values, input.value_count) }?;
        total_values = total_values
            .checked_add(handles.len())
            .ok_or_else(collection_limit)?;
        if total_values > MAX_CANONICAL_COLLECTION_LEN {
            return Err(collection_limit());
        }
        if keys_only && handles.len() != 1 {
            return Err(invalid_shape_diagnostic(
                "c_projected_reference_key_cardinality_invalid",
                "Each projected reference key requires exactly one scalar",
            ));
        }
        let mut values = Vec::with_capacity(handles.len());
        for handle in handles {
            if handle.is_null() {
                return Err(descriptor_invalid());
            }
            // SAFETY: caller retains every immutable value handle during this call.
            let value = unsafe { &*handle };
            if !same_package_brand(package, value.package()) {
                return Err(invalid_brand_diagnostic());
            }
            total_bytes = total_bytes
                .checked_add(projected_value_bytes(value.value())?)
                .ok_or_else(collection_limit)?;
            if total_bytes > MAX_PROJECTED_MODEL_BYTES {
                return Err(collection_limit());
            }
            values.push(value.value().clone());
        }
        output.push((field, values));
    }
    Ok(output)
}

unsafe fn collect_roles(
    package: &Arc<SchemaPackageState>,
    owner: &type_bridge_contract::id::TypeId,
    inputs: *const TypeBridgeProjectedRoleInputV1,
    count: usize,
) -> Result<
    Vec<(type_bridge_contract::id::RoleId, Vec<ProjectedReference>)>,
    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
> {
    validate_array(inputs.cast(), count)?;
    let mut output = Vec::with_capacity(count);
    let mut total_references = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 0..count {
        // SAFETY: caller promises all bounded descriptor elements are readable.
        let input = unsafe { inputs.add(index).read_unaligned() };
        if !validate_layout(
            input.struct_size,
            size_of::<TypeBridgeProjectedRoleInputV1>(),
            input.version,
            input.reserved,
        ) {
            return Err(descriptor_invalid());
        }
        // SAFETY: generated token storage is readable for this call.
        let (role_owner, role) = unsafe { resolve_role_token(package, input.role) }?;
        if &role_owner != owner {
            return Err(invalid_shape_diagnostic(
                "c_projected_role_owner_mismatch",
                "The projected role token belongs to a different model",
            ));
        }
        // SAFETY: pointer-array validation and snapshotting are centralized.
        let handles = unsafe { snapshot_handle_array(input.references, input.reference_count) }?;
        total_references = total_references
            .checked_add(handles.len())
            .ok_or_else(collection_limit)?;
        if total_references > MAX_CANONICAL_COLLECTION_LEN {
            return Err(collection_limit());
        }
        let mut references = Vec::with_capacity(handles.len());
        for handle in handles {
            if handle.is_null() {
                return Err(descriptor_invalid());
            }
            // SAFETY: caller retains every immutable reference handle during this call.
            let reference = unsafe { &*handle };
            if !same_package_brand(package, &reference.package) {
                return Err(invalid_brand_diagnostic());
            }
            total_bytes = total_bytes
                .checked_add(projected_reference_bytes(&reference.value)?)
                .ok_or_else(collection_limit)?;
            if total_bytes > MAX_PROJECTED_MODEL_BYTES {
                return Err(collection_limit());
            }
            references.push(reference.value.clone());
        }
        output.push((role, references));
    }
    Ok(output)
}

pub(crate) fn wrap_value(
    package: &Arc<SchemaPackageState>,
    value: ProjectedAttributeValue,
) -> TypeBridgeProjectedValue {
    TypeBridgeProjectedValue::from_owned(Arc::clone(package), value)
}

fn wrap_reference(
    package: &Arc<SchemaPackageState>,
    value: ProjectedReference,
) -> TypeBridgeProjectedReference {
    TypeBridgeProjectedReference {
        package: Arc::clone(package),
        value,
    }
}

fn write_value_handle(
    value: TypeBridgeProjectedValue,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let value = match try_box(AllocationSite::ProjectedValueHandle, value) {
        Ok(value) => value,
        Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
    };
    // SAFETY: caller output was initialized and remains writable.
    unsafe { out_value.write_unaligned(Box::into_raw(value)) };
    TypeBridgeStatus::Ok
}

fn write_reference_handle(
    reference: TypeBridgeProjectedReference,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let reference = match try_box(AllocationSite::ProjectedReferenceHandle, reference) {
        Ok(value) => value,
        Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
    };
    // SAFETY: caller output was initialized and remains writable.
    unsafe { out_reference.write_unaligned(Box::into_raw(reference)) };
    TypeBridgeStatus::Ok
}

fn projected_model_output_preflight<T>(
    out_value: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[
        (out_value.cast(), size_of::<*mut T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])
}

unsafe fn preflight_field_graph(
    preflight: &DirectOutputPreflight,
    fields: *const TypeBridgeProjectedFieldInputV1,
    field_count: usize,
    total_handles: &mut usize,
) -> Result<(), TypeBridgeStatus> {
    if field_count > MAX_CANONICAL_COLLECTION_LEN {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    if field_count == 0 || fields.is_null() {
        return Ok(());
    }
    let storage_bytes = field_count
        .checked_mul(size_of::<TypeBridgeProjectedFieldInputV1>())
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    preflight.check_bytes(fields.cast(), storage_bytes)?;

    for index in 0..field_count {
        // SAFETY: the caller promises this bounded descriptor array is readable.
        let field = unsafe { fields.add(index).read_unaligned() };
        if !validate_layout(
            field.struct_size,
            size_of::<TypeBridgeProjectedFieldInputV1>(),
            field.version,
            field.reserved,
        ) {
            return Ok(());
        }
        *total_handles = total_handles
            .checked_add(field.value_count)
            .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
            .ok_or(TypeBridgeStatus::ResourceLimit)?;
        if !field.values.is_null() {
            for value_index in 0..field.value_count {
                // SAFETY: the caller promises the bounded pointer-array storage is readable.
                let value = unsafe { field.values.add(value_index).read_unaligned() };
                if !value.is_null() {
                    // SAFETY: the caller retains every complete immutable handle for this call.
                    let nested = unsafe { &*value }.value.resource_measure().members();
                    *total_handles = total_handles
                        .checked_add(nested.saturating_sub(1))
                        .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
                        .ok_or(TypeBridgeStatus::ResourceLimit)?;
                }
            }
        }
    }
    for index in 0..field_count {
        // SAFETY: layout validation above established this readable element shape.
        let field = unsafe { fields.add(index).read_unaligned() };
        preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, field.field.cast())?;
        if field.value_count != 0 && !field.values.is_null() {
            // SAFETY: the bounded pointer-array storage and pointees remain
            // caller-owned and immutable for the complete constructor call.
            unsafe { preflight.check_pointer_array(field.values, field.value_count) }?;
            for value_index in 0..field.value_count {
                // SAFETY: the pointer-array storage and complete pointees were checked above.
                let value = unsafe { field.values.add(value_index).read_unaligned() };
                if !value.is_null() {
                    // SAFETY: this immutable handle remains live throughout preflight.
                    unsafe { &*value }.check_borrowed_ranges(preflight)?;
                }
            }
        }
    }
    Ok(())
}

unsafe fn preflight_role_graph(
    preflight: &DirectOutputPreflight,
    roles: *const TypeBridgeProjectedRoleInputV1,
    role_count: usize,
    total_handles: &mut usize,
) -> Result<(), TypeBridgeStatus> {
    if role_count > MAX_CANONICAL_COLLECTION_LEN {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    if role_count == 0 || roles.is_null() {
        return Ok(());
    }
    let storage_bytes = role_count
        .checked_mul(size_of::<TypeBridgeProjectedRoleInputV1>())
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    preflight.check_bytes(roles.cast(), storage_bytes)?;

    for index in 0..role_count {
        // SAFETY: the caller promises this bounded descriptor array is readable.
        let role = unsafe { roles.add(index).read_unaligned() };
        if !validate_layout(
            role.struct_size,
            size_of::<TypeBridgeProjectedRoleInputV1>(),
            role.version,
            role.reserved,
        ) {
            return Ok(());
        }
        *total_handles = total_handles
            .checked_add(role.reference_count)
            .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
            .ok_or(TypeBridgeStatus::ResourceLimit)?;
        if !role.references.is_null() {
            for reference_index in 0..role.reference_count {
                // SAFETY: the caller promises the bounded pointer-array storage is readable.
                let reference = unsafe { role.references.add(reference_index).read_unaligned() };
                if !reference.is_null() {
                    // SAFETY: the caller retains every complete immutable handle for this call.
                    let nested = unsafe { &*reference }.value.resource_measure().members();
                    *total_handles = total_handles
                        .checked_add(nested.saturating_sub(1))
                        .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
                        .ok_or(TypeBridgeStatus::ResourceLimit)?;
                }
            }
        }
    }
    for index in 0..role_count {
        // SAFETY: layout validation above established this readable element shape.
        let role = unsafe { roles.add(index).read_unaligned() };
        preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, role.role.cast())?;
        if role.reference_count != 0 && !role.references.is_null() {
            // SAFETY: the bounded pointer-array storage and pointees remain
            // caller-owned and immutable for the complete constructor call.
            unsafe { preflight.check_pointer_array(role.references, role.reference_count) }?;
            for reference_index in 0..role.reference_count {
                // SAFETY: the pointer-array storage and complete pointees were checked above.
                let reference = unsafe { role.references.add(reference_index).read_unaligned() };
                if !reference.is_null() {
                    // SAFETY: this immutable handle remains live throughout preflight.
                    unsafe { &*reference }.check_borrowed_ranges(preflight)?;
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn preflight_projected_model_graph(
    preflight: &DirectOutputPreflight,
    package: *const TypeBridgeSchemaPackage,
    descriptor: *const (),
    descriptor_size: usize,
    model: *const TypeBridgeProjectedTokenV1,
    iid: Option<TypeBridgeByteView>,
    fields: *const TypeBridgeProjectedFieldInputV1,
    field_count: usize,
    roles: *const TypeBridgeProjectedRoleInputV1,
    role_count: usize,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())?;
    if !package.is_null() {
        // SAFETY: the complete package handle object is disjoint from all outputs.
        let package = unsafe { &*package };
        preflight.check_package_borrowed_ranges(package.state())?;
    }
    if !descriptor.is_null() {
        preflight.check_bytes(descriptor.cast(), descriptor_size)?;
    }
    preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast())?;
    if let Some(iid) = iid {
        preflight.check_bytes(iid.data.cast(), iid.length)?;
    }
    let mut total_handles = 1_usize
        .checked_add(field_count)
        .and_then(|count| count.checked_add(role_count))
        .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    // SAFETY: the top-level descriptor snapshot supplied these bounded arrays.
    unsafe { preflight_field_graph(preflight, fields, field_count, &mut total_handles) }?;
    // SAFETY: the top-level descriptor snapshot supplied these bounded arrays.
    unsafe { preflight_role_graph(preflight, roles, role_count, &mut total_handles) }?;
    Ok(())
}

/// Construct an immutable projected reference and copy every input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_open_v1(
    package: *const TypeBridgeSchemaPackage,
    descriptor: *const TypeBridgeProjectedReferenceDescriptorV1,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match projected_model_output_preflight(out_reference, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = unsafe {
        preflight_projected_model_graph(
            &preflight,
            package,
            std::ptr::null(),
            0,
            std::ptr::null(),
            None,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
        )
    } {
        return status;
    }
    if !descriptor.is_null() {
        // SAFETY: top-level descriptor storage is caller-readable for this call.
        let snapshot = unsafe { descriptor.read_unaligned() };
        if validate_layout(
            snapshot.struct_size,
            size_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
            snapshot.version,
            snapshot.reserved,
        ) {
            // SAFETY: all graph ranges are inspected read-only before output initialization.
            if let Err(status) = unsafe {
                preflight_projected_model_graph(
                    &preflight,
                    package,
                    descriptor.cast(),
                    size_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
                    snapshot.model,
                    Some(snapshot.iid),
                    snapshot.keys,
                    snapshot.key_count,
                    std::ptr::null(),
                    0,
                )
            } {
                return status;
            }
        } else if let Err(status) = preflight.check_bytes(
            descriptor.cast(),
            size_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
        ) {
            return status;
        }
    } else if let Err(status) =
        preflight.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())
    {
        return status;
    }
    // SAFETY: independent outputs are initialized before any input inspection.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() || descriptor.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live package and readable descriptor during this call.
        let package = unsafe { &*package };
        let descriptor = unsafe { descriptor.read_unaligned() };
        if !validate_layout(
            descriptor.struct_size,
            size_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
            descriptor.version,
            descriptor.reserved,
        ) {
            return return_execution_error(descriptor_invalid(), out_diagnostics);
        }
        // SAFETY: generated token and byte storage are readable for this call.
        let model = match unsafe { resolve_model_token(package.state(), descriptor.model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let iid = match unsafe { copied_optional_iid(descriptor.iid) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: nested descriptors and handles are snapshotted during this call.
        let keys = match unsafe {
            collect_fields(
                package.state(),
                &model,
                descriptor.keys,
                descriptor.key_count,
                true,
            )
        } {
            Ok(values) => values
                .into_iter()
                .map(|(field, mut values)| {
                    (
                        field,
                        values
                            .pop()
                            .expect("key cardinality was checked before collection"),
                    )
                })
                .collect(),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let reference = match ProjectedReference::try_new(
            &package.state().installed_projection,
            model,
            iid,
            keys,
        ) {
            Ok(value) => wrap_reference(package.state(), value),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        write_reference_handle(reference, out_reference, out_diagnostics)
    })
}

/// Construct an immutable projected create value and copy every input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_open_v1(
    package: *const TypeBridgeSchemaPackage,
    descriptor: *const TypeBridgeProjectedCreateDescriptorV1,
    out_create: *mut *mut TypeBridgeProjectedCreate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match projected_model_output_preflight(out_create, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = unsafe {
        preflight_projected_model_graph(
            &preflight,
            package,
            std::ptr::null(),
            0,
            std::ptr::null(),
            None,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
        )
    } {
        return status;
    }
    if !descriptor.is_null() {
        // SAFETY: top-level descriptor storage is caller-readable for this call.
        let snapshot = unsafe { descriptor.read_unaligned() };
        if validate_layout(
            snapshot.struct_size,
            size_of::<TypeBridgeProjectedCreateDescriptorV1>(),
            snapshot.version,
            snapshot.reserved,
        ) {
            // SAFETY: all graph ranges are inspected read-only before output initialization.
            if let Err(status) = unsafe {
                preflight_projected_model_graph(
                    &preflight,
                    package,
                    descriptor.cast(),
                    size_of::<TypeBridgeProjectedCreateDescriptorV1>(),
                    snapshot.model,
                    None,
                    snapshot.fields,
                    snapshot.field_count,
                    snapshot.roles,
                    snapshot.role_count,
                )
            } {
                return status;
            }
        } else if let Err(status) = preflight.check_bytes(
            descriptor.cast(),
            size_of::<TypeBridgeProjectedCreateDescriptorV1>(),
        ) {
            return status;
        }
    } else if let Err(status) =
        preflight.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())
    {
        return status;
    }
    // SAFETY: independent outputs are initialized before any input inspection.
    if let Err(status) = unsafe { initialize_execution_outputs(out_create, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() || descriptor.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live package and readable descriptor during this call.
        let package = unsafe { &*package };
        let descriptor = unsafe { descriptor.read_unaligned() };
        if !validate_layout(
            descriptor.struct_size,
            size_of::<TypeBridgeProjectedCreateDescriptorV1>(),
            descriptor.version,
            descriptor.reserved,
        ) {
            return return_execution_error(descriptor_invalid(), out_diagnostics);
        }
        // SAFETY: generated token storage is readable for this call.
        let model = match unsafe { resolve_model_token(package.state(), descriptor.model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: nested descriptors and handles are snapshotted during this call.
        let fields = match unsafe {
            collect_fields(
                package.state(),
                &model,
                descriptor.fields,
                descriptor.field_count,
                false,
            )
        } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: nested descriptors and handles are snapshotted during this call.
        let roles = match unsafe {
            collect_roles(
                package.state(),
                &model,
                descriptor.roles,
                descriptor.role_count,
            )
        } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let create = match ProjectedCreate::try_new(
            &package.state().installed_projection,
            model,
            fields,
            roles,
        ) {
            Ok(value) => TypeBridgeProjectedCreate {
                package: Arc::clone(package.state()),
                value,
            },
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let create = match try_box(AllocationSite::ProjectedCreateHandle, create) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_create.write_unaligned(Box::into_raw(create)) };
        TypeBridgeStatus::Ok
    })
}

/// Construct an immutable completely hydrated projected thing and copy every input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_open_v1(
    package: *const TypeBridgeSchemaPackage,
    descriptor: *const TypeBridgeProjectedThingDescriptorV1,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match projected_model_output_preflight(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = unsafe {
        preflight_projected_model_graph(
            &preflight,
            package,
            std::ptr::null(),
            0,
            std::ptr::null(),
            None,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
        )
    } {
        return status;
    }
    if !descriptor.is_null() {
        // SAFETY: top-level descriptor storage is caller-readable for this call.
        let snapshot = unsafe { descriptor.read_unaligned() };
        if validate_layout(
            snapshot.struct_size,
            size_of::<TypeBridgeProjectedThingDescriptorV1>(),
            snapshot.version,
            snapshot.reserved,
        ) {
            // SAFETY: all graph ranges are inspected read-only before output initialization.
            if let Err(status) = unsafe {
                preflight_projected_model_graph(
                    &preflight,
                    package,
                    descriptor.cast(),
                    size_of::<TypeBridgeProjectedThingDescriptorV1>(),
                    snapshot.model,
                    Some(snapshot.iid),
                    snapshot.fields,
                    snapshot.field_count,
                    snapshot.roles,
                    snapshot.role_count,
                )
            } {
                return status;
            }
        } else if let Err(status) = preflight.check_bytes(
            descriptor.cast(),
            size_of::<TypeBridgeProjectedThingDescriptorV1>(),
        ) {
            return status;
        }
    } else if let Err(status) =
        preflight.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())
    {
        return status;
    }
    // SAFETY: independent outputs are initialized before any input inspection.
    if let Err(status) = unsafe { initialize_execution_outputs(out_thing, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() || descriptor.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live package and readable descriptor during this call.
        let package = unsafe { &*package };
        let descriptor = unsafe { descriptor.read_unaligned() };
        if !validate_layout(
            descriptor.struct_size,
            size_of::<TypeBridgeProjectedThingDescriptorV1>(),
            descriptor.version,
            descriptor.reserved,
        ) {
            return return_execution_error(descriptor_invalid(), out_diagnostics);
        }
        // SAFETY: generated token and IID storage are readable for this call.
        let model = match unsafe { resolve_model_token(package.state(), descriptor.model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let iid = match unsafe { copied_optional_iid(descriptor.iid) } {
            Ok(Some(value)) => value,
            Ok(None) => return return_execution_error(descriptor_invalid(), out_diagnostics),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: nested descriptors and handles are snapshotted during this call.
        let fields = match unsafe {
            collect_fields(
                package.state(),
                &model,
                descriptor.fields,
                descriptor.field_count,
                false,
            )
        } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: nested descriptors and handles are snapshotted during this call.
        let hydrated_roles = match unsafe {
            collect_roles(
                package.state(),
                &model,
                descriptor.roles,
                descriptor.role_count,
            )
        } {
            Ok(value) => value
                .into_iter()
                .map(|(role, references)| {
                    references
                        .into_iter()
                        .map(|reference| {
                            ProjectedRolePlayer::try_new(
                                &package.state().installed_projection,
                                reference,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map(|players| (role, players))
                })
                .collect::<Result<Vec<_>, _>>(),
            Err(diagnostic) => Err(diagnostic),
        };
        let roles = match hydrated_roles {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let thing = match ProjectedThing::try_new(
            &package.state().installed_projection,
            model,
            iid,
            fields,
            roles,
        ) {
            Ok(value) => TypeBridgeProjectedThing {
                package: Arc::clone(package.state()),
                value: Arc::new(value),
            },
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let thing = match try_box(AllocationSite::ProjectedThingHandle, thing) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_thing.write_unaligned(Box::into_raw(thing)) };
        TypeBridgeStatus::Ok
    })
}

unsafe fn resolve_field_for_handle(
    package: &Arc<SchemaPackageState>,
    expected_owner: &type_bridge_contract::id::TypeId,
    field: *const TypeBridgeProjectedTokenV1,
) -> Result<OwnsFactId, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is readable for this call.
    let (owner, field) = unsafe { resolve_field_token(package, field) }?;
    if &owner != expected_owner {
        return Err(invalid_shape_diagnostic(
            "c_projected_field_owner_mismatch",
            "The projected field token belongs to a different model",
        ));
    }
    Ok(field)
}

unsafe fn resolve_role_for_handle(
    package: &Arc<SchemaPackageState>,
    expected_owner: &type_bridge_contract::id::TypeId,
    role: *const TypeBridgeProjectedTokenV1,
) -> Result<
    type_bridge_contract::id::RoleId,
    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
> {
    // SAFETY: generated token storage is readable for this call.
    let (owner, role) = unsafe { resolve_role_token(package, role) }?;
    if &owner != expected_owner {
        return Err(invalid_shape_diagnostic(
            "c_projected_role_owner_mismatch",
            "The projected role token belongs to a different model",
        ));
    }
    Ok(role)
}

fn write_count(out_count: *mut usize, count: usize) -> TypeBridgeStatus {
    if out_count.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller supplied one writable output slot.
    unsafe { out_count.write_unaligned(count) };
    TypeBridgeStatus::Ok
}

fn preflight_model_accessor(
    inputs: &[(u32, *const std::ffi::c_void)],
    outputs: &[(*mut std::ffi::c_void, usize)],
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = direct_output_preflight(outputs)?;
    for &(kind, pointer) in inputs {
        preflight.check_object_kind(kind, pointer)?;
    }
    Ok(preflight)
}

unsafe fn preflight_handle_accessor<T: ProjectedHandleBorrowedRanges>(
    handle: *const T,
    handle_kind: u32,
    token: *const TypeBridgeProjectedTokenV1,
    outputs: &[(*mut std::ffi::c_void, usize)],
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = preflight_model_accessor(
        &[
            (handle_kind, handle.cast()),
            (GENERATED_INPUT_PROJECTED_TOKEN, token.cast()),
        ],
        outputs,
    )?;
    // SAFETY: the complete opaque handle object is disjoint from every output.
    unsafe { check_handle_borrowed_ranges(handle, &preflight) }?;
    Ok(preflight)
}

unsafe fn initialize_count_and_diagnostics(
    _preflight: &DirectOutputPreflight,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if !out_count.is_null() {
        // SAFETY: input aliasing was rejected before this unaligned caller write.
        unsafe { out_count.write_unaligned(0) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: input aliasing was rejected before this unaligned caller write.
        unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    }
    if out_count.is_null() || out_diagnostics.is_null() {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

macro_rules! field_count_accessor {
    ($function:ident, $handle:ty, $handle_kind:expr) => {
        #[doc = "Return the number of values for one generated field token."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            handle: *const $handle,
            field: *const TypeBridgeProjectedTokenV1,
            out_count: *mut usize,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            let preflight = match preflight_model_accessor(
                &[
                    ($handle_kind, handle.cast()),
                    (GENERATED_INPUT_PROJECTED_TOKEN, field.cast()),
                ],
                &[
                    (out_count.cast(), size_of::<usize>()),
                    (
                        out_diagnostics.cast(),
                        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                    ),
                ],
            ) {
                Ok(value) => value,
                Err(status) => return status,
            };
            // SAFETY: the complete opaque handle was checked above.
            if let Err(status) = unsafe { check_handle_borrowed_ranges(handle, &preflight) } {
                return status;
            }
            // SAFETY: the complete live handle and token ranges are disjoint.
            if let Err(status) =
                unsafe { initialize_count_and_diagnostics(&preflight, out_count, out_diagnostics) }
            {
                return status;
            }
            guarded(|| {
                if handle.is_null() {
                    return TypeBridgeStatus::InvalidArgument;
                }
                // SAFETY: caller retains a live immutable handle during this call.
                let handle = unsafe { &*handle };
                // SAFETY: generated token storage is readable for this call.
                let field = match unsafe {
                    resolve_field_for_handle(&handle.package, handle.value.type_id(), field)
                } {
                    Ok(value) => value,
                    Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
                };
                write_count(
                    out_count,
                    handle.value.fields().get(&field).map_or(0, Vec::len),
                )
            })
        }
    };
}

field_count_accessor!(
    type_bridge_projected_create_field_count,
    TypeBridgeProjectedCreate,
    GENERATED_INPUT_PROJECTED_CREATE
);
field_count_accessor!(
    type_bridge_projected_thing_field_count,
    TypeBridgeProjectedThing,
    GENERATED_INPUT_PROJECTED_THING
);

unsafe fn field_value_at(
    package: &Arc<SchemaPackageState>,
    owner: &type_bridge_contract::id::TypeId,
    fields: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    field: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: generated token storage is readable for this call.
    let field = match unsafe { resolve_field_for_handle(package, owner, field) } {
        Ok(value) => value,
        Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
    };
    let Some(value) = fields
        .get(&field)
        .and_then(|values| values.get(index))
        .cloned()
    else {
        return return_execution_error(index_out_of_bounds(), out_diagnostics);
    };
    let value = wrap_value(package, value);
    write_value_handle(value, out_value, out_diagnostics)
}

macro_rules! field_value_accessor {
    ($function:ident, $handle:ty, $handle_kind:expr) => {
        #[doc = "Clone one generated field value into a new owned scalar handle."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            handle: *const $handle,
            field: *const TypeBridgeProjectedTokenV1,
            index: usize,
            out_value: *mut *mut TypeBridgeProjectedValue,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            let preflight = match preflight_model_accessor(
                &[
                    ($handle_kind, handle.cast()),
                    (GENERATED_INPUT_PROJECTED_TOKEN, field.cast()),
                ],
                &[
                    (out_value.cast(), size_of::<*mut TypeBridgeProjectedValue>()),
                    (
                        out_diagnostics.cast(),
                        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                    ),
                ],
            ) {
                Ok(value) => value,
                Err(status) => return status,
            };
            // SAFETY: the complete opaque handle was checked above.
            if let Err(status) = unsafe { check_handle_borrowed_ranges(handle, &preflight) } {
                return status;
            }
            // SAFETY: independent outputs are initialized after input preflight.
            if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) }
            {
                return status;
            }
            guarded(|| {
                if handle.is_null() {
                    return TypeBridgeStatus::InvalidArgument;
                }
                // SAFETY: caller retains a live immutable handle during this call.
                let handle = unsafe { &*handle };
                // SAFETY: token validation and output ownership are centralized.
                unsafe {
                    field_value_at(
                        &handle.package,
                        handle.value.type_id(),
                        handle.value.fields(),
                        field,
                        index,
                        out_value,
                        out_diagnostics,
                    )
                }
            })
        }
    };
}

field_value_accessor!(
    type_bridge_projected_create_field_value_at,
    TypeBridgeProjectedCreate,
    GENERATED_INPUT_PROJECTED_CREATE
);
field_value_accessor!(
    type_bridge_projected_thing_field_value_at,
    TypeBridgeProjectedThing,
    GENERATED_INPUT_PROJECTED_THING
);

/// Clone one scalar complete-model field, returning null for an absent optional value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_scalar_field_value(
    thing: *const TypeBridgeProjectedThing,
    field: *const TypeBridgeProjectedTokenV1,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_model_accessor(
        &[
            (GENERATED_INPUT_PROJECTED_THING, thing.cast()),
            (GENERATED_INPUT_PROJECTED_TOKEN, field.cast()),
        ],
        &[
            (out_value.cast(), size_of::<*mut TypeBridgeProjectedValue>()),
            (
                out_diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ],
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the complete opaque thing handle was checked above.
    if let Err(status) = unsafe { check_handle_borrowed_ranges(thing, &preflight) } {
        return status;
    }
    // SAFETY: independent outputs are initialized after complete-range preflight.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable complete-model handle.
        let thing = unsafe { &*thing };
        // SAFETY: generated token storage remains caller-readable for this call.
        let field =
            match unsafe { resolve_field_for_handle(&thing.package, thing.value.type_id(), field) }
            {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let Some(model) = thing
            .package
            .installed_projection
            .projection()
            .models()
            .get(thing.value.type_id())
        else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_thing_model_missing",
                    "The hydrated thing model is absent from its retained projection",
                ),
                out_diagnostics,
            );
        };
        let Some(read_field) = model
            .complete_read()
            .fields()
            .iter()
            .find(|candidate| candidate.token() == &field)
        else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_thing_field_missing",
                    "The hydrated scalar field is absent from the complete model projection",
                ),
                out_diagnostics,
            );
        };
        if read_field.multiplicity().container() != ProjectedContainer::Scalar {
            return return_execution_error(
                invalid_shape_diagnostic(
                    "c_projected_thing_field_not_scalar",
                    "The projected thing field is a sequence rather than a scalar",
                ),
                out_diagnostics,
            );
        }
        let values = thing
            .value
            .fields()
            .get(&field)
            .map_or(&[][..], Vec::as_slice);
        let value = match values {
            [] if !read_field.multiplicity().required() => return TypeBridgeStatus::Ok,
            [value] => value.clone(),
            [] => {
                return return_execution_error(
                    scalar_hydration_integrity(
                        "c_projected_thing_required_scalar_missing",
                        "The hydrated thing is missing a required projected scalar",
                    ),
                    out_diagnostics,
                );
            }
            _ => {
                return return_execution_error(
                    scalar_hydration_integrity(
                        "c_projected_thing_scalar_cardinality_invalid",
                        "The hydrated thing contains more than one value for a projected scalar",
                    ),
                    out_diagnostics,
                );
            }
        };
        let value = wrap_value(&thing.package, value);
        write_value_handle(value, out_value, out_diagnostics)
    })
}

/// Clone one scalar complete-model role player, returning null when optional and absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_scalar_role_reference(
    thing: *const TypeBridgeProjectedThing,
    role: *const TypeBridgeProjectedTokenV1,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            role,
            &[
                (
                    out_reference.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: preflight proved both outputs disjoint from both live inputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable hydrated thing during this call.
        let thing = unsafe { &*thing };
        // SAFETY: generated token storage remains caller-readable for this call.
        let role =
            match unsafe { resolve_role_for_handle(&thing.package, thing.value.type_id(), role) } {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let Some(model) = thing
            .package
            .installed_projection
            .projection()
            .models()
            .get(thing.value.type_id())
        else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_thing_model_missing",
                    "The hydrated thing model is absent from its retained projection",
                ),
                out_diagnostics,
            );
        };
        let Some(read_role) = model.complete_read().roles().get(&role) else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_thing_role_missing",
                    "The hydrated scalar role is absent from the complete model projection",
                ),
                out_diagnostics,
            );
        };
        if read_role.multiplicity().container() != ProjectedContainer::Scalar {
            return return_execution_error(
                invalid_shape_diagnostic(
                    "c_projected_thing_role_not_scalar",
                    "The projected thing role is a sequence rather than a scalar",
                ),
                out_diagnostics,
            );
        }
        let players = thing
            .value
            .roles()
            .get(&role)
            .map_or(&[][..], Vec::as_slice);
        let reference = match players {
            [] if !read_role.multiplicity().required() => return TypeBridgeStatus::Ok,
            [player] => player.reference().clone(),
            [] => {
                return return_execution_error(
                    scalar_hydration_integrity(
                        "c_projected_thing_required_scalar_role_missing",
                        "The hydrated thing is missing a required projected scalar role",
                    ),
                    out_diagnostics,
                );
            }
            _ => {
                return return_execution_error(
                    scalar_hydration_integrity(
                        "c_projected_thing_scalar_role_cardinality_invalid",
                        "The hydrated thing contains more than one player for a scalar role",
                    ),
                    out_diagnostics,
                );
            }
        };
        let reference = wrap_reference(&thing.package, reference);
        write_reference_handle(reference, out_reference, out_diagnostics)
    })
}

unsafe fn validate_exact_model(
    package: &Arc<SchemaPackageState>,
    actual: &type_bridge_contract::id::TypeId,
    model: *const TypeBridgeProjectedTokenV1,
    mismatch: fn() -> type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the generated model token remains readable for this call and is
    // resolved against the immutable package retained by the handle.
    let expected = match unsafe { resolve_model_token(package, model) } {
        Ok(value) => value,
        Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
    };
    if &expected != actual {
        return return_execution_error(mismatch(), out_diagnostics);
    }
    TypeBridgeStatus::Ok
}

/// Validate a projected reference against its generated exact-model token.
///
/// This generated-only fence is read-only and performs no provider I/O.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_validate_model(
    reference: *const TypeBridgeProjectedReference,
    model: *const TypeBridgeProjectedTokenV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            model,
            &[(
                out_diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            )],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight proved the diagnostics slot does not overlap a live input.
    unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    guarded(|| {
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains this immutable reference for the call.
        let reference = unsafe { &*reference };
        // SAFETY: token resolution and exact model comparison are centralized.
        unsafe {
            validate_exact_model(
                &reference.package,
                reference.value.type_id(),
                model,
                reference_model_mismatch,
                out_diagnostics,
            )
        }
    })
}

/// Validate that a projected reference belongs to one generated role's closed player domain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_validate_role(
    reference: *const TypeBridgeProjectedReference,
    role: *const TypeBridgeProjectedTokenV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            role,
            &[(
                out_diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            )],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight proved the diagnostics slot disjoint from both inputs.
    unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    guarded(|| {
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains this immutable reference for the call.
        let reference = unsafe { &*reference };
        if let Err(diagnostic) = reference
            .value
            .validate_for(&reference.package.installed_projection)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        // SAFETY: generated token storage remains caller-readable for this call.
        let (owner, role_id) = match unsafe { resolve_role_token(&reference.package, role) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let Some(role_projection) = reference
            .package
            .installed_projection
            .projection()
            .models()
            .get(&owner)
            .and_then(|model| model.query_tokens().roles().get(&role_id))
        else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_reference_role_token_missing",
                    "The generated role token is absent from its retained model projection",
                ),
                out_diagnostics,
            );
        };
        if !role_projection
            .accepted_players()
            .contains(reference.value.type_id())
        {
            return return_execution_error(reference_role_player_mismatch(), out_diagnostics);
        }
        TypeBridgeStatus::Ok
    })
}

/// Clone one projected reference while preserving its package and database origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_clone(
    reference: *const TypeBridgeProjectedReference,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            std::ptr::null(),
            &[
                (
                    out_reference.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: preflight proved both caller slots disjoint from the live input.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable reference for the duration of the clone.
        let reference = unsafe { &*reference };
        if let Err(diagnostic) = reference
            .value
            .validate_for(&reference.package.installed_projection)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let cloned = TypeBridgeProjectedReference {
            package: Arc::clone(&reference.package),
            value: reference.value.clone(),
        };
        write_reference_handle(cloned, out_reference, out_diagnostics)
    })
}

/// Return the retained projection's generated model-token ordinal for a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_model_ordinal(
    reference: *const TypeBridgeProjectedReference,
    out_ordinal: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            std::ptr::null(),
            &[
                (out_ordinal.cast(), size_of::<u32>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if !out_ordinal.is_null() {
        // SAFETY: preflight proved this caller slot disjoint from the input.
        unsafe { out_ordinal.write_unaligned(0) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: preflight proved this caller slot disjoint from the input.
        unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    }
    if out_ordinal.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable reference during the read-only call.
        let reference = unsafe { &*reference };
        if let Err(diagnostic) = reference
            .value
            .validate_for(&reference.package.installed_projection)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let identity = ProjectedTokenIdentity::Model(reference.value.type_id().clone());
        let Some(ordinal) = reference
            .package
            .installed_projection
            .projection()
            .projected_token_ordinal(&identity)
        else {
            return return_execution_error(
                scalar_hydration_integrity(
                    "c_projected_reference_model_token_missing",
                    "The projected reference model has no generated token ordinal",
                ),
                out_diagnostics,
            );
        };
        // SAFETY: the initialized output remains caller-writable.
        unsafe { out_ordinal.write_unaligned(ordinal) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow the canonical IID of a projected reference, or an empty view for a key-only reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_iid(
    reference: *const TypeBridgeProjectedReference,
    out_iid: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            std::ptr::null(),
            &[(out_iid.cast(), size_of::<TypeBridgeByteView>())],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        // SAFETY: initialize output before inspecting the handle.
        if let Err(status) = unsafe { initialize_view(out_iid) } {
            return status;
        }
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let reference = unsafe { &*reference };
        let Some(iid) = reference.value.iid() else {
            return TypeBridgeStatus::Ok;
        };
        match borrowed_view(iid.as_bytes(), out_iid) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Borrow the mandatory canonical IID of a completely hydrated projected thing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_iid(
    thing: *const TypeBridgeProjectedThing,
    out_iid: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            std::ptr::null(),
            &[(out_iid.cast(), size_of::<TypeBridgeByteView>())],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        // SAFETY: initialize output before inspecting the handle.
        if let Err(status) = unsafe { initialize_view(out_iid) } {
            return status;
        }
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let thing = unsafe { &*thing };
        match borrowed_view(thing.value.iid().as_bytes(), out_iid) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Validate a projected hydrated thing against its generated exact-model token.
///
/// This generated-only fence is read-only and performs no provider I/O.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_validate_model(
    thing: *const TypeBridgeProjectedThing,
    model: *const TypeBridgeProjectedTokenV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            model,
            &[(
                out_diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            )],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight proved the diagnostics slot does not overlap a live input.
    unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains this immutable thing for the call.
        let thing = unsafe { &*thing };
        // SAFETY: token resolution and exact model comparison are centralized.
        unsafe {
            validate_exact_model(
                &thing.package,
                thing.value.type_id(),
                model,
                thing_model_mismatch,
                out_diagnostics,
            )
        }
    })
}

/// Derive an immutable reference that preserves a thing's IID, exact keys, and opaque origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_reference(
    thing: *const TypeBridgeProjectedThing,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            std::ptr::null(),
            &[
                (
                    out_reference.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: independent outputs are initialized after complete-range preflight.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable thing handle during this call.
        let thing = unsafe { &*thing };
        let reference = match thing
            .value
            .try_to_reference(&thing.package.installed_projection)
        {
            Ok(value) => wrap_reference(&thing.package, value),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        write_reference_handle(reference, out_reference, out_diagnostics)
    })
}

unsafe fn role_reference_at(
    package: &Arc<SchemaPackageState>,
    owner: &type_bridge_contract::id::TypeId,
    roles: &BTreeMap<type_bridge_contract::id::RoleId, Vec<ProjectedReference>>,
    role: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: generated token storage is readable for this call.
    let role = match unsafe { resolve_role_for_handle(package, owner, role) } {
        Ok(value) => value,
        Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
    };
    let Some(reference) = roles
        .get(&role)
        .and_then(|references| references.get(index))
        .cloned()
    else {
        return return_execution_error(index_out_of_bounds(), out_diagnostics);
    };
    let reference = wrap_reference(package, reference);
    write_reference_handle(reference, out_reference, out_diagnostics)
}

/// Return the number of create references for one generated role token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_role_count(
    create: *const TypeBridgeProjectedCreate,
    role: *const TypeBridgeProjectedTokenV1,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let preflight = match unsafe {
        preflight_handle_accessor(
            create,
            GENERATED_INPUT_PROJECTED_CREATE,
            role,
            &[
                (out_count.cast(), size_of::<usize>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: every live input range is disjoint from both caller outputs.
    if let Err(status) =
        unsafe { initialize_count_and_diagnostics(&preflight, out_count, out_diagnostics) }
    {
        return status;
    }
    guarded(|| {
        if create.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let create = unsafe { &*create };
        // SAFETY: generated token storage is readable for this call.
        let role =
            match unsafe { resolve_role_for_handle(&create.package, create.value.type_id(), role) }
            {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        write_count(
            out_count,
            create.value.roles().get(&role).map_or(0, Vec::len),
        )
    })
}

/// Clone one create role reference into a new owned handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_create_role_reference_at(
    create: *const TypeBridgeProjectedCreate,
    role: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            create,
            GENERATED_INPUT_PROJECTED_CREATE,
            role,
            &[
                (
                    out_reference.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: independent outputs are initialized after complete-range preflight.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if create.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let create = unsafe { &*create };
        // SAFETY: token validation and output ownership are centralized.
        unsafe {
            role_reference_at(
                &create.package,
                create.value.type_id(),
                create.value.roles(),
                role,
                index,
                out_reference,
                out_diagnostics,
            )
        }
    })
}

/// Return the number of hydrated players for one generated role token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_role_count(
    thing: *const TypeBridgeProjectedThing,
    role: *const TypeBridgeProjectedTokenV1,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            role,
            &[
                (out_count.cast(), size_of::<usize>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: every live input range is disjoint from both caller outputs.
    if let Err(status) =
        unsafe { initialize_count_and_diagnostics(&preflight, out_count, out_diagnostics) }
    {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let thing = unsafe { &*thing };
        // SAFETY: generated token storage is readable for this call.
        let role =
            match unsafe { resolve_role_for_handle(&thing.package, thing.value.type_id(), role) } {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        write_count(
            out_count,
            thing.value.roles().get(&role).map_or(0, Vec::len),
        )
    })
}

/// Clone one hydrated role player into a new owned reference handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_role_reference_at(
    thing: *const TypeBridgeProjectedThing,
    role: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out_reference: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            thing,
            GENERATED_INPUT_PROJECTED_THING,
            role,
            &[
                (
                    out_reference.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: independent outputs are initialized after complete-range preflight.
    if let Err(status) = unsafe { initialize_execution_outputs(out_reference, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let thing = unsafe { &*thing };
        // SAFETY: generated token storage is readable for this call.
        let role =
            match unsafe { resolve_role_for_handle(&thing.package, thing.value.type_id(), role) } {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let Some(player) = thing
            .value
            .roles()
            .get(&role)
            .and_then(|players| players.get(index))
        else {
            return return_execution_error(index_out_of_bounds(), out_diagnostics);
        };
        let reference = wrap_reference(&thing.package, player.reference().clone());
        write_reference_handle(reference, out_reference, out_diagnostics)
    })
}

/// Return one reference key scalar as a new owned projected-value handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_reference_key(
    reference: *const TypeBridgeProjectedReference,
    field: *const TypeBridgeProjectedTokenV1,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer and exposed borrowed ranges are checked read-only.
    let _preflight = match unsafe {
        preflight_handle_accessor(
            reference,
            GENERATED_INPUT_PROJECTED_REFERENCE,
            field,
            &[
                (out_value.cast(), size_of::<*mut TypeBridgeProjectedValue>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: independent outputs are initialized after complete-range preflight.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if reference.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let reference = unsafe { &*reference };
        // SAFETY: generated token storage is readable for this call.
        let field = match unsafe {
            resolve_field_for_handle(&reference.package, reference.value.type_id(), field)
        } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let Some(value) = reference.value.keys().get(&field).cloned() else {
            return return_execution_error(reference_key_missing(), out_diagnostics);
        };
        let value = wrap_value(&reference.package, value);
        write_value_handle(value, out_value, out_diagnostics)
    })
}

macro_rules! close_handle {
    ($function:ident, $type:ty, $doc:literal) => {
        #[doc = $doc]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(handle: *mut *mut $type) -> TypeBridgeStatus {
            // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the header.
            unsafe { close_box(handle) }
        }
    };
}

close_handle!(
    type_bridge_projected_reference_close,
    TypeBridgeProjectedReference,
    "Close one projected reference and clear its slot."
);
close_handle!(
    type_bridge_projected_create_close,
    TypeBridgeProjectedCreate,
    "Close one projected create value and clear its slot."
);
close_handle!(
    type_bridge_projected_thing_close,
    TypeBridgeProjectedThing,
    "Close one projected thing and clear its slot."
);
