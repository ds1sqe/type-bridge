//! Private ABI 1.6 owned canonical bytes and deterministic archive handles.

use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH,
    MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::projected_record::{
    MAX_PROJECTED_ARCHIVE_BYTES, MAX_PROJECTED_ARCHIVE_RECORDS, MAX_PROJECTED_DECODED_WEIGHT,
    MAX_PROJECTED_RECORD_BYTES, ProjectedArchive, ProjectedRecord,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_contract::value::CanonicalValue;

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus,
    borrowed_view, close_box, guarded, initialize_view,
};
use crate::allocation::{
    AllocationSite, allocation_checkpoint, allocation_exhausted, try_box, try_reserve,
};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_CANONICAL_ARCHIVE,
    GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER, GENERATED_INPUT_CANONICAL_BYTES,
    GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, GENERATED_INPUT_PROJECTED_CREATE,
    GENERATED_INPUT_PROJECTED_REFERENCE, GENERATED_INPUT_PROJECTED_STRUCT,
    GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, GENERATED_INPUT_PROJECTED_THING,
    GENERATED_INPUT_PROJECTED_TOKEN, GENERATED_INPUT_PROJECTED_VALUE,
    GENERATED_INPUT_SCHEMA_PACKAGE, direct_output_preflight,
};
use crate::projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedReference, TypeBridgeProjectedThing,
};
use crate::projected_token::{
    TypeBridgeProjectedTokenV1, resolve_attribute_token, resolve_model_token, resolve_struct_token,
};
use crate::projected_value::{
    InlineCanonicalText, TypeBridgeProjectedValue, TypeBridgeProjectedValueKind,
};
use crate::runtime::TypeBridgeCancellation;

const PROJECTED_CODEC_OPTIONS_V1: u32 = 1;
const PROJECTED_CODEC_HAS_TIMEOUT: u32 = 1;

/// Versioned tighten-only controls for canonical record and archive work.
#[repr(C)]
pub struct TypeBridgeProjectedCodecOptionsV1 {
    /// Must equal `sizeof(TypeBridgeProjectedCodecOptionsV1)`.
    pub struct_size: u64,
    /// Must be `1`.
    pub version: u32,
    /// Bit 0 declares `timeout_milliseconds` present; all other bits are zero.
    pub flags: u32,
    /// Relative timeout converted once to an absolute monotonic deadline.
    pub timeout_milliseconds: u64,
    /// Tighten-only canonical input byte ceiling.
    pub max_input_bytes: u64,
    /// Tighten-only canonical output byte ceiling.
    pub max_output_bytes: u64,
    /// Tighten-only canonical nesting depth ceiling.
    pub max_depth: u64,
    /// Tighten-only ordered archive record ceiling.
    pub max_records: u64,
    /// Tighten-only decoded member/value/reference weight ceiling.
    pub max_members: u64,
    /// Optional separately owned common cancellation handle.
    pub cancellation: *const TypeBridgeCancellation,
}

#[derive(Clone)]
pub struct CanonicalControl {
    pub deadline: Option<Instant>,
    pub cancellation: type_bridge_orm::AnswerCancellation,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_depth: usize,
    pub max_records: usize,
    pub max_members: usize,
}

impl CanonicalControl {
    pub fn check(&self) -> Result<(), SdkExecutionDiagnostic> {
        if self.cancellation.is_cancelled() {
            Err(SdkExecutionDiagnostic::data_operation_cancelled())
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(SdkExecutionDiagnostic::data_operation_deadline_exceeded())
        } else {
            Ok(())
        }
    }

    fn input_limits(&self) -> CodecLimits {
        CodecLimits {
            max_bytes: self.max_input_bytes,
            max_depth: self.max_depth,
            max_collection_len: self.max_members,
            max_string_bytes: MAX_CANONICAL_STRING_BYTES.min(self.max_input_bytes),
        }
    }

    fn output_limits(&self) -> CodecLimits {
        CodecLimits {
            max_bytes: self.max_output_bytes,
            max_depth: self.max_depth,
            max_collection_len: self.max_members,
            max_string_bytes: MAX_CANONICAL_STRING_BYTES.min(self.max_output_bytes),
        }
    }
}

pub unsafe fn canonical_control(
    options: *const TypeBridgeProjectedCodecOptionsV1,
    archive: bool,
) -> Result<CanonicalControl, TypeBridgeStatus> {
    let (max_input_bytes, max_output_bytes, max_records) = if archive {
        (
            MAX_PROJECTED_ARCHIVE_BYTES,
            MAX_PROJECTED_ARCHIVE_BYTES,
            MAX_PROJECTED_ARCHIVE_RECORDS,
        )
    } else {
        (MAX_PROJECTED_RECORD_BYTES, MAX_PROJECTED_RECORD_BYTES, 1)
    };
    if options.is_null() {
        return Ok(CanonicalControl {
            deadline: None,
            cancellation: type_bridge_orm::AnswerCancellation::default(),
            max_input_bytes,
            max_output_bytes,
            max_depth: MAX_CANONICAL_DEPTH,
            max_records,
            max_members: MAX_PROJECTED_DECODED_WEIGHT,
        });
    }
    // SAFETY: the caller retains one complete possibly unaligned options object.
    let options = unsafe { options.read_unaligned() };
    if options.struct_size != size_of::<TypeBridgeProjectedCodecOptionsV1>() as u64
        || options.version != PROJECTED_CODEC_OPTIONS_V1
        || options.flags & !PROJECTED_CODEC_HAS_TIMEOUT != 0
    {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    let limit = |value: u64, ceiling: usize| {
        usize::try_from(value)
            .map(|value| value.min(ceiling))
            .map_err(|_| TypeBridgeStatus::ResourceLimit)
    };
    let deadline = if options.flags & PROJECTED_CODEC_HAS_TIMEOUT != 0 {
        Some(
            Instant::now()
                .checked_add(Duration::from_millis(options.timeout_milliseconds))
                .ok_or(TypeBridgeStatus::InvalidArgument)?,
        )
    } else {
        None
    };
    let cancellation = if options.cancellation.is_null() {
        type_bridge_orm::AnswerCancellation::default()
    } else {
        // SAFETY: the caller retains the cancellation handle while controls are captured.
        unsafe { &*options.cancellation }.answer_cancellation()
    };
    Ok(CanonicalControl {
        deadline,
        cancellation,
        max_input_bytes: limit(options.max_input_bytes, max_input_bytes)?,
        max_output_bytes: limit(options.max_output_bytes, max_output_bytes)?,
        max_depth: limit(options.max_depth, MAX_CANONICAL_DEPTH)?,
        max_records: limit(options.max_records, max_records)?,
        max_members: limit(
            options.max_members,
            MAX_PROJECTED_DECODED_WEIGHT.min(MAX_CANONICAL_COLLECTION_LEN),
        )?,
    })
}

/// Opaque owned immutable canonical byte buffer.
pub struct TypeBridgeCanonicalBytes {
    bytes: Vec<u8>,
}

/// Opaque bounded deterministic archive builder.
pub struct TypeBridgeCanonicalArchiveBuilder {
    package: Arc<SchemaPackageState>,
    control: CanonicalControl,
    records: Vec<ProjectedRecord>,
}

/// Opaque verified immutable canonical archive.
pub struct TypeBridgeCanonicalArchive {
    package: Arc<SchemaPackageState>,
    control: CanonicalControl,
    archive: ProjectedArchive,
}

/// Opaque immutable package-branded projected struct value.
pub struct TypeBridgeProjectedStruct {
    package: Arc<SchemaPackageState>,
    value: type_bridge_orm::ProjectedStructValue,
}

/// Opaque independently owned canonical member from a projected struct.
pub struct TypeBridgeProjectedStructMember {
    value: CanonicalValue,
    canonical_text: Option<InlineCanonicalText>,
}

impl TypeBridgeProjectedStructMember {
    fn new(value: CanonicalValue) -> Self {
        let canonical_text = InlineCanonicalText::new(&value);
        Self {
            value,
            canonical_text,
        }
    }

    fn kind(&self) -> TypeBridgeProjectedValueKind {
        match &self.value {
            CanonicalValue::String(_) => TypeBridgeProjectedValueKind::String,
            CanonicalValue::Long(_) => TypeBridgeProjectedValueKind::Long,
            CanonicalValue::Double(_) => TypeBridgeProjectedValueKind::Double,
            CanonicalValue::Boolean(_) => TypeBridgeProjectedValueKind::Boolean,
            CanonicalValue::Date(_) => TypeBridgeProjectedValueKind::Date,
            CanonicalValue::DateTime(_) => TypeBridgeProjectedValueKind::DateTime,
            CanonicalValue::DateTimeTz(_) => TypeBridgeProjectedValueKind::DateTimeTz,
            CanonicalValue::Decimal(_) => TypeBridgeProjectedValueKind::Decimal,
            CanonicalValue::Duration(_) => TypeBridgeProjectedValueKind::Duration,
        }
    }

    fn text(&self) -> Option<&[u8]> {
        match &self.value {
            CanonicalValue::String(value) => Some(value.as_str().as_bytes()),
            CanonicalValue::Decimal(value) => Some(value.as_str().as_bytes()),
            CanonicalValue::Date(_)
            | CanonicalValue::DateTime(_)
            | CanonicalValue::DateTimeTz(_)
            | CanonicalValue::Duration(_) => self
                .canonical_text
                .as_ref()
                .map(InlineCanonicalText::as_bytes),
            CanonicalValue::Long(_) | CanonicalValue::Double(_) | CanonicalValue::Boolean(_) => {
                None
            }
        }
    }

    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        if let Some(text) = self.text() {
            preflight.check_bytes(text.as_ptr().cast(), text.len())?;
        }
        Ok(())
    }
}

impl TypeBridgeCanonicalBytes {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_bytes(self.bytes.as_ptr().cast(), self.bytes.len())
    }
}

impl TypeBridgeCanonicalArchiveBuilder {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        preflight.check_bytes(
            self.records.as_ptr().cast(),
            size_of_val(self.records.as_slice()),
        )
    }
}

impl TypeBridgeCanonicalArchive {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)
    }
}

impl TypeBridgeProjectedStruct {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        preflight.check_bytes(
            self.value.members().as_ptr().cast(),
            size_of_val(self.value.members()),
        )
    }
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C canonical archive code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C canonical archive message is valid")
}

fn invalid_record() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_record_invalid"),
        message("Canonical record bytes are invalid for the installed schema package"),
    )
}

fn invalid_archive() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_archive_invalid"),
        message("Canonical archive bytes are invalid for the installed schema package"),
    )
}

fn canonical_input_limit() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        code("c_canonical_input_limit"),
        message("Canonical input bytes exceed the supported bounded allocation"),
    )
}

fn canonical_output_limit() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        code("c_canonical_output_limit"),
        message("Canonical output bytes exceed the supported bounded allocation"),
    )
}

fn invalid_value() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_value_invalid"),
        message("Projected value cannot be encoded by this installed schema package"),
    )
}

fn wrong_record_kind() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_record_kind_mismatch"),
        message("Canonical record kind differs from the requested generated result kind"),
    )
}

fn wrong_record_type() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_record_type_mismatch"),
        message("Canonical record type differs from the requested generated token"),
    )
}

fn invalid_struct_member() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_struct_member_invalid"),
        message("Projected struct member index is outside the exact generated declaration"),
    )
}

unsafe fn preflight(
    outputs: &[(*mut c_void, usize)],
    objects: &[(u32, *const c_void)],
    bytes: &[TypeBridgeByteView],
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = direct_output_preflight(outputs)?;
    for &(kind, pointer) in objects {
        // SAFETY: each ABI caller promises a live object of its declared kind.
        unsafe { preflight.check_deep_object_kind(kind, pointer) }?;
    }
    for view in bytes {
        preflight.check_bytes(view.data.cast(), view.length)?;
    }
    Ok(preflight)
}

unsafe fn decode_record(
    package: *const TypeBridgeSchemaPackage,
    bytes: TypeBridgeByteView,
    control: &CanonicalControl,
) -> Result<(Arc<SchemaPackageState>, ProjectedRecord), SdkExecutionDiagnostic> {
    if package.is_null() {
        return Err(invalid_record());
    }
    control.check()?;
    // SAFETY: caller retains one live immutable package for this call.
    let package = unsafe { &*package };
    // SAFETY: caller retains the bounded byte range for this call.
    let bytes = unsafe { snapshot_bytes(bytes, control.max_input_bytes, invalid_record) }?;
    control.check()?;
    let record = ProjectedRecord::decode_with_limits(&bytes, control.input_limits())
        .map_err(|_| invalid_record())?;
    if record.decoded_weight() > control.max_members {
        return Err(canonical_input_limit());
    }
    control.check()?;
    Ok((Arc::clone(package.state()), record))
}

fn publish_record(
    record: Result<ProjectedRecord, type_bridge_orm::ProjectedCodecError>,
    control: &CanonicalControl,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if let Err(diagnostic) = control.check() {
        return return_execution_error(diagnostic, out_diagnostics);
    }
    let record = match record {
        Ok(record) => record,
        Err(_) => return return_execution_error(invalid_value(), out_diagnostics),
    };
    let bytes = match encode_bytes(
        AllocationSite::CanonicalRecordEncodeBytes,
        || record.encode_with_limits(control.output_limits()),
        invalid_value,
    ) {
        Ok(bytes) => bytes,
        Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
    };
    if bytes.len() > control.max_output_bytes {
        return return_execution_error(canonical_output_limit(), out_diagnostics);
    }
    if let Err(diagnostic) = control.check() {
        return return_execution_error(diagnostic, out_diagnostics);
    }
    let bytes = match try_box(
        AllocationSite::CanonicalBytesHandle,
        TypeBridgeCanonicalBytes { bytes },
    ) {
        Ok(bytes) => bytes,
        Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
    };
    // SAFETY: the shared output initializer proved this slot writable.
    unsafe { out_bytes.write_unaligned(Box::into_raw(bytes)) };
    TypeBridgeStatus::Ok
}

fn encode_bytes(
    site: AllocationSite,
    encode: impl FnOnce() -> Result<Vec<u8>, type_bridge_contract::diagnostic::Diagnostic>,
    invalid: fn() -> SdkExecutionDiagnostic,
) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
    allocation_checkpoint(site).map_err(|_| allocation_exhausted())?;
    encode().map_err(|_| invalid())
}

/// Encode one exact package-branded projected attribute value as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_attribute_v1(
    value: *const TypeBridgeProjectedValue,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the projected value while complete alias ranges are inspected.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_VALUE, value.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected value for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_attribute(
                &value.package.installed_projection,
                &value.value,
            ),
            &control,
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded projected create as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_create_v1(
    value: *const TypeBridgeProjectedCreate,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the projected create while complete alias ranges are inspected.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_CREATE, value.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected create for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_create(&value.package.installed_projection, &value.value),
            &control,
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded detached reference as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_reference_v1(
    value: *const TypeBridgeProjectedReference,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the projected reference while alias ranges are inspected.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_REFERENCE, value.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected reference for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_reference(
                &value.package.installed_projection,
                &value.value,
            ),
            &control,
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded hydrated thing as a detached canonical snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_snapshot_v1(
    value: *const TypeBridgeProjectedThing,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the projected thing while complete alias ranges are inspected.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_THING, value.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected thing for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_snapshot(
                &value.package.installed_projection,
                &value.value,
            ),
            &control,
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded generated struct as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_struct_v1(
    value: *const TypeBridgeProjectedStruct,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains the projected struct while complete alias ranges are inspected.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_STRUCT, value.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected struct for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_struct(&value.package.installed_projection, &value.value),
            &control,
            out_bytes,
            out_diagnostics,
        )
    })
}

fn publish_decoded<T>(
    site: AllocationSite,
    value: T,
    out_value: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let value = match try_box(site, value) {
        Ok(value) => value,
        Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
    };
    // SAFETY: the shared output initializer proved this slot writable.
    unsafe { out_value.write_unaligned(Box::into_raw(value)) };
    TypeBridgeStatus::Ok
}

fn materialize(
    package: &SchemaPackageState,
    record: &ProjectedRecord,
) -> Result<type_bridge_orm::ProjectedCodecValue, SdkExecutionDiagnostic> {
    type_bridge_orm::materialize_record(&package.installed_projection, record)
        .map_err(|_| invalid_record())
}

/// Decode an exact generated attribute-field record without publishing on mismatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_decode_attribute_v1(
    package: *const TypeBridgeSchemaPackage,
    bytes: TypeBridgeByteView,
    expected_attribute: *const TypeBridgeProjectedTokenV1,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: package, token, and byte inputs remain live during read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_value.cast(), size_of::<*mut TypeBridgeProjectedValue>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                (GENERATED_INPUT_PROJECTED_TOKEN, expected_attribute.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[bytes],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        // SAFETY: package and byte ranges remain caller-owned for this call.
        let (package, record) = match unsafe { decode_record(package, bytes, &control) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: generated token storage remains readable for this call.
        let attribute = match unsafe { resolve_attribute_token(&package, expected_attribute) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let expected = match TypeId::new(TypeKind::Attribute, attribute.label().as_str()) {
            Ok(value) => value,
            Err(_) => return return_execution_error(wrong_record_type(), out_diagnostics),
        };
        let type_bridge_contract::projected_record::ProjectedRecordContent::AttributeValue {
            r#type,
            ..
        } = record.content()
        else {
            return return_execution_error(wrong_record_kind(), out_diagnostics);
        };
        if r#type != &expected {
            return return_execution_error(wrong_record_type(), out_diagnostics);
        }
        let value = match materialize(&package, &record) {
            Ok(type_bridge_orm::ProjectedCodecValue::Attribute(value)) => value,
            Ok(_) => return return_execution_error(wrong_record_kind(), out_diagnostics),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        publish_decoded(
            AllocationSite::ProjectedValueHandle,
            TypeBridgeProjectedValue::from_owned(package, value),
            out_value,
            out_diagnostics,
        )
    })
}

macro_rules! decode_model_record {
    ($function:ident, $variant:ident, $type_id:ident, $kind_pattern:pat, $output:ty, $site:expr, $wrap:expr) => {
        #[doc = "Decode one exact generated model record without publishing on mismatch."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            package: *const TypeBridgeSchemaPackage,
            bytes: TypeBridgeByteView,
            expected_model: *const TypeBridgeProjectedTokenV1,
            options: *const TypeBridgeProjectedCodecOptionsV1,
            out_value: *mut *mut $output,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: package, token, and byte inputs remain live during read-only preflight.
            if let Err(status) = unsafe {
                preflight(
                    &[
                        (out_value.cast(), size_of::<*mut $output>()),
                        (
                            out_diagnostics.cast(),
                            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                        ),
                    ],
                    &[
                        (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                        (GENERATED_INPUT_PROJECTED_TOKEN, expected_model.cast()),
                        (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
                    ],
                    &[bytes],
                )
            } {
                return status;
            }
            // SAFETY: shared initializer validates and clears distinct writable outputs.
            if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) }
            {
                return status;
            }
            guarded(|| {
                let control = match unsafe { canonical_control(options, false) } {
                    Ok(control) => control,
                    Err(status) => return status,
                };
                // SAFETY: package and byte ranges remain caller-owned for this call.
                let (package, record) = match unsafe { decode_record(package, bytes, &control) } {
                    Ok(value) => value,
                    Err(diagnostic) => {
                        return return_execution_error(diagnostic, out_diagnostics);
                    }
                };
                // SAFETY: generated token storage remains readable for this call.
                let expected = match unsafe { resolve_model_token(&package, expected_model) } {
                    Ok(value) => value,
                    Err(diagnostic) => {
                        return return_execution_error(diagnostic, out_diagnostics);
                    }
                };
                let $kind_pattern = record.content() else {
                    return return_execution_error(wrong_record_kind(), out_diagnostics);
                };
                if $type_id != &expected {
                    return return_execution_error(wrong_record_type(), out_diagnostics);
                }
                let value = match materialize(&package, &record) {
                    Ok(type_bridge_orm::ProjectedCodecValue::$variant(value)) => value,
                    Ok(_) => return return_execution_error(wrong_record_kind(), out_diagnostics),
                    Err(diagnostic) => {
                        return return_execution_error(diagnostic, out_diagnostics);
                    }
                };
                publish_decoded($site, ($wrap)(package, value), out_value, out_diagnostics)
            })
        }
    };
}

decode_model_record!(
    type_bridge_canonical_record_decode_create_v1,
    Create,
    record_type,
    (type_bridge_contract::projected_record::ProjectedRecordContent::EntityCreate {
        r#type: record_type,
        ..
    } | type_bridge_contract::projected_record::ProjectedRecordContent::RelationCreate {
        r#type: record_type,
        ..
    }),
    TypeBridgeProjectedCreate,
    AllocationSite::ProjectedCreateHandle,
    |package, value| TypeBridgeProjectedCreate { package, value }
);

decode_model_record!(
    type_bridge_canonical_record_decode_snapshot_v1,
    Snapshot,
    record_type,
    (type_bridge_contract::projected_record::ProjectedRecordContent::EntitySnapshot {
        r#type: record_type,
        ..
    } | type_bridge_contract::projected_record::ProjectedRecordContent::RelationSnapshot {
        r#type: record_type,
        ..
    }),
    TypeBridgeProjectedThing,
    AllocationSite::ProjectedThingHandle,
    |package, value| TypeBridgeProjectedThing {
        package,
        value: Arc::new(value),
    }
);

/// Decode one exact generated reference record without publishing on mismatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_decode_reference_v1(
    package: *const TypeBridgeSchemaPackage,
    bytes: TypeBridgeByteView,
    expected_model: *const TypeBridgeProjectedTokenV1,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_value: *mut *mut TypeBridgeProjectedReference,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: package, token, and byte inputs remain live during read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    out_value.cast(),
                    size_of::<*mut TypeBridgeProjectedReference>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                (GENERATED_INPUT_PROJECTED_TOKEN, expected_model.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[bytes],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        // SAFETY: package and byte ranges remain caller-owned for this call.
        let (package, record) = match unsafe { decode_record(package, bytes, &control) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: generated token storage remains readable for this call.
        let expected = match unsafe { resolve_model_token(&package, expected_model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let type_bridge_contract::projected_record::ProjectedRecordContent::Reference { reference } =
            record.content()
        else {
            return return_execution_error(wrong_record_kind(), out_diagnostics);
        };
        if reference.type_id() != &expected {
            return return_execution_error(wrong_record_type(), out_diagnostics);
        }
        let value = match materialize(&package, &record) {
            Ok(type_bridge_orm::ProjectedCodecValue::Reference(value)) => value,
            Ok(_) => return return_execution_error(wrong_record_kind(), out_diagnostics),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        publish_decoded(
            AllocationSite::ProjectedReferenceHandle,
            TypeBridgeProjectedReference { package, value },
            out_value,
            out_diagnostics,
        )
    })
}

/// Decode one exact generated struct record without publishing on mismatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_decode_struct_v1(
    package: *const TypeBridgeSchemaPackage,
    bytes: TypeBridgeByteView,
    expected_struct: *const TypeBridgeProjectedTokenV1,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_value: *mut *mut TypeBridgeProjectedStruct,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: package, token, and byte inputs remain live during read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    out_value.cast(),
                    size_of::<*mut TypeBridgeProjectedStruct>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                (GENERATED_INPUT_PROJECTED_TOKEN, expected_struct.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[bytes],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, false) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        // SAFETY: package and byte ranges remain caller-owned for this call.
        let (package, record) = match unsafe { decode_record(package, bytes, &control) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: generated token storage remains readable for this call.
        let expected = match unsafe { resolve_struct_token(&package, expected_struct) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let type_bridge_contract::projected_record::ProjectedRecordContent::StructValue {
            r#type,
            ..
        } = record.content()
        else {
            return return_execution_error(wrong_record_kind(), out_diagnostics);
        };
        if r#type.label() != expected.label() {
            return return_execution_error(wrong_record_type(), out_diagnostics);
        }
        let value = match materialize(&package, &record) {
            Ok(type_bridge_orm::ProjectedCodecValue::Struct(value)) => value,
            Ok(_) => return return_execution_error(wrong_record_kind(), out_diagnostics),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        publish_decoded(
            AllocationSite::ProjectedStructHandle,
            TypeBridgeProjectedStruct { package, value },
            out_value,
            out_diagnostics,
        )
    })
}

/// Close one decoded projected struct and clear its ownership slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_close(
    value: *mut *mut TypeBridgeProjectedStruct,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(value) }
}

/// Return one independently owned present member from an exact generated struct.
///
/// An absent optional member returns `OK` with a null member output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_at_v1(
    value: *const TypeBridgeProjectedStruct,
    expected_struct: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out_member: *mut *mut TypeBridgeProjectedStructMember,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: struct and token remain live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    out_member.cast(),
                    size_of::<*mut TypeBridgeProjectedStructMember>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_PROJECTED_STRUCT, value.cast()),
                (GENERATED_INPUT_PROJECTED_TOKEN, expected_struct.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_member, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable struct handle for this call.
        let value = unsafe { &*value };
        // SAFETY: generated token storage remains readable for this call.
        let expected = match unsafe { resolve_struct_token(&value.package, expected_struct) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if value.value.type_id().label() != expected.label() {
            return return_execution_error(wrong_record_type(), out_diagnostics);
        }
        let Some(member) = value.value.members().get(index) else {
            return return_execution_error(invalid_struct_member(), out_diagnostics);
        };
        let Some(member) = member.clone() else {
            return TypeBridgeStatus::Ok;
        };
        publish_decoded(
            AllocationSite::ProjectedStructMemberHandle,
            TypeBridgeProjectedStructMember::new(member),
            out_member,
            out_diagnostics,
        )
    })
}

/// Return the stable scalar kind of one independently owned struct member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_kind(
    value: *const TypeBridgeProjectedStructMember,
    out_kind: *mut TypeBridgeProjectedValueKind,
) -> TypeBridgeStatus {
    // SAFETY: member remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_kind.cast(), size_of::<TypeBridgeProjectedValueKind>())],
            &[(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, value.cast())],
            &[],
        )
    } {
        return status;
    }
    guarded(|| {
        if out_kind.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable scalar output.
        unsafe { out_kind.write_unaligned(TypeBridgeProjectedValueKind::String) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable member handle for this call.
        unsafe { out_kind.write_unaligned((&*value).kind()) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow canonical lexical text from a textual or temporal struct member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_text(
    value: *const TypeBridgeProjectedStructMember,
    out_text: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: member remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_text.cast(), size_of::<TypeBridgeByteView>())],
            &[(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, value.cast())],
            &[],
        )
    } {
        return status;
    }
    guarded(|| {
        // SAFETY: initialize the complete view before inspecting the input.
        if let Err(status) = unsafe { initialize_view(out_text) } {
            return status;
        }
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable member handle for this call.
        let Some(text) = (unsafe { &*value }).text() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match borrowed_view(text, out_text) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Return one exact signed 64-bit struct member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_long(
    value: *const TypeBridgeProjectedStructMember,
    out_value: *mut i64,
) -> TypeBridgeStatus {
    // SAFETY: member remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_value.cast(), size_of::<i64>())],
            &[(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, value.cast())],
            &[],
        )
    } {
        return status;
    }
    guarded(|| {
        if out_value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable scalar output.
        unsafe { out_value.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable member handle for this call.
        match unsafe { &(*value).value } {
            CanonicalValue::Long(value) => {
                // SAFETY: output was validated and initialized above.
                unsafe { out_value.write_unaligned(*value) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Return exact finite IEEE-754 binary64 bits from a struct member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_double_bits(
    value: *const TypeBridgeProjectedStructMember,
    out_bits: *mut u64,
) -> TypeBridgeStatus {
    // SAFETY: member remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_bits.cast(), size_of::<u64>())],
            &[(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, value.cast())],
            &[],
        )
    } {
        return status;
    }
    guarded(|| {
        if out_bits.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable scalar output.
        unsafe { out_bits.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable member handle for this call.
        match unsafe { &(*value).value } {
            CanonicalValue::Double(value) => {
                // SAFETY: output was validated and initialized above.
                unsafe { out_bits.write_unaligned(value.bits()) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Return an exact zero-or-one Boolean struct member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_boolean(
    value: *const TypeBridgeProjectedStructMember,
    out_boolean: *mut u8,
) -> TypeBridgeStatus {
    // SAFETY: member remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_boolean.cast(), size_of::<u8>())],
            &[(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, value.cast())],
            &[],
        )
    } {
        return status;
    }
    guarded(|| {
        if out_boolean.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable scalar output.
        unsafe { out_boolean.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable member handle for this call.
        match unsafe { &(*value).value } {
            CanonicalValue::Boolean(value) => {
                // SAFETY: output was validated and initialized above.
                unsafe { out_boolean.write_unaligned(u8::from(*value)) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Close one independently owned projected struct member and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_struct_member_close(
    value: *mut *mut TypeBridgeProjectedStructMember,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(value) }
}

unsafe fn snapshot_bytes(
    view: TypeBridgeByteView,
    max_input_bytes: usize,
    invalid: fn() -> SdkExecutionDiagnostic,
) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
    if view.data.is_null() || view.length == 0 {
        return Err(invalid());
    }
    if view.length > max_input_bytes.min(MAX_CANONICAL_BYTES) {
        return Err(canonical_input_limit());
    }
    let mut snapshot = Vec::new();
    try_reserve(
        &mut snapshot,
        view.length,
        AllocationSite::CanonicalInputBytes,
    )
    .map_err(|_| allocation_exhausted())?;
    // SAFETY: the C contract requires the non-null input range to remain readable
    // for this call; u8 has alignment one and the stable ceiling bounds the copy.
    snapshot.extend_from_slice(unsafe { std::slice::from_raw_parts(view.data, view.length) });
    Ok(snapshot)
}

fn validate_record(
    package: &SchemaPackageState,
    bytes: &[u8],
) -> Result<ProjectedRecord, SdkExecutionDiagnostic> {
    let record = ProjectedRecord::decode(bytes).map_err(|_| invalid_record())?;
    type_bridge_orm::materialize_record(&package.installed_projection, &record)
        .map_err(|_| invalid_record())?;
    Ok(record)
}

fn reserve_archive_record(records: &mut Vec<ProjectedRecord>) -> Result<(), ()> {
    try_reserve(records, 1, AllocationSite::CanonicalArchiveBuilderRecords).map_err(|_| ())
}

fn clone_archive_records(records: &[ProjectedRecord]) -> Result<Vec<ProjectedRecord>, ()> {
    let mut cloned = Vec::new();
    try_reserve(
        &mut cloned,
        records.len(),
        AllocationSite::CanonicalArchiveFinishRecords,
    )
    .map_err(|_| ())?;
    cloned.extend(records.iter().cloned());
    Ok(cloned)
}

/// Open an empty archive builder bound to one exact installed package.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_builder_open_v1(
    package: *const TypeBridgeSchemaPackage,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: package remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    out_builder.cast(),
                    size_of::<*mut TypeBridgeCanonicalArchiveBuilder>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_builder, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, true) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller retains a live immutable package for this call.
        let package = unsafe { &*package };
        let builder = TypeBridgeCanonicalArchiveBuilder {
            package: Arc::clone(package.state()),
            control,
            records: Vec::new(),
        };
        let builder = match try_box(AllocationSite::CanonicalArchiveBuilderHandle, builder) {
            Ok(builder) => builder,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: the output slot was cleared and validated above.
        unsafe { out_builder.write_unaligned(Box::into_raw(builder)) };
        TypeBridgeStatus::Ok
    })
}

/// Copy and append one complete canonical record to an archive builder.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_builder_append_record_v1(
    builder: *mut TypeBridgeCanonicalArchiveBuilder,
    record: TypeBridgeByteView,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: builder and record bytes remain live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(
                (out_diagnostics.cast()),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            )],
            &[(GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER, builder.cast())],
            &[record],
        )
    } {
        return status;
    }
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller supplies one writable diagnostics slot.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        if builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the builder is uniquely borrowed for this mutating call.
        let builder = unsafe { &mut *builder };
        // SAFETY: the caller retains the byte range for this call.
        let control = &builder.control;
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let bytes = match unsafe { snapshot_bytes(record, control.max_input_bytes, invalid_record) }
        {
            Ok(bytes) => bytes,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let record = match validate_record(&builder.package, &bytes) {
            Ok(record) => record,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if builder.records.len() >= control.max_records {
            return return_execution_error(canonical_input_limit(), out_diagnostics);
        }
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if reserve_archive_record(&mut builder.records).is_err() {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        builder.records.push(record);
        TypeBridgeStatus::Ok
    })
}

/// Finish a builder and publish one owned deterministic archive buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_builder_finish_v1(
    builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller supplies a readable ownership slot for this call.
    let retained = unsafe { builder.read_unaligned() };
    // SAFETY: retained builder remains live until successful atomic consumption.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    builder.cast(),
                    size_of::<*mut TypeBridgeCanonicalArchiveBuilder>(),
                ),
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[(GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER, retained.cast())],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if retained.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the retained builder remains owned by the caller until success.
        let value = unsafe { &*retained };
        let control = &value.control;
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if value.records.len() > control.max_records {
            return return_execution_error(canonical_input_limit(), out_diagnostics);
        }
        let records = match clone_archive_records(&value.records) {
            Ok(records) => records,
            Err(()) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let archive = match ProjectedArchive::try_new(records) {
            Ok(archive) => archive,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
        };
        let bytes = match encode_bytes(
            AllocationSite::CanonicalArchiveEncodeBytes,
            || archive.encode_with_limits(control.output_limits()),
            invalid_archive,
        ) {
            Ok(bytes) => bytes,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let bytes = match try_box(
            AllocationSite::CanonicalBytesHandle,
            TypeBridgeCanonicalBytes { bytes },
        ) {
            Ok(bytes) => bytes,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: all fallible work completed; consume and clear the builder atomically.
        unsafe {
            builder.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(retained));
            out_bytes.write_unaligned(Box::into_raw(bytes));
        }
        TypeBridgeStatus::Ok
    })
}

/// Open and fully validate one immutable archive against exact package authority.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_open_v1(
    package: *const TypeBridgeSchemaPackage,
    bytes: TypeBridgeByteView,
    options: *const TypeBridgeProjectedCodecOptionsV1,
    out_archive: *mut *mut TypeBridgeCanonicalArchive,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: package and byte range remain live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (
                    out_archive.cast(),
                    size_of::<*mut TypeBridgeCanonicalArchive>(),
                ),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[
                (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
                (GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, options.cast()),
            ],
            &[bytes],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_archive, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let control = match unsafe { canonical_control(options, true) } {
            Ok(control) => control,
            Err(status) => return status,
        };
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both input objects for this call.
        let package = unsafe { &*package };
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let bytes = match unsafe { snapshot_bytes(bytes, control.max_input_bytes, invalid_archive) }
        {
            Ok(bytes) => bytes,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let archive = match ProjectedArchive::decode_with_limits(&bytes, control.input_limits()) {
            Ok(archive) => archive,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
        };
        if archive.records().len() > control.max_records
            || archive.decoded_weight() > control.max_members
        {
            return return_execution_error(canonical_input_limit(), out_diagnostics);
        }
        for record in archive.records() {
            if type_bridge_orm::materialize_record(&package.state().installed_projection, record)
                .is_err()
            {
                return return_execution_error(invalid_archive(), out_diagnostics);
            }
        }
        let archive = TypeBridgeCanonicalArchive {
            package: Arc::clone(package.state()),
            control,
            archive,
        };
        let archive = match try_box(AllocationSite::CanonicalArchiveHandle, archive) {
            Ok(archive) => archive,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: the output slot was cleared and validated above.
        unsafe { out_archive.write_unaligned(Box::into_raw(archive)) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow the immutable bytes retained by an owned canonical byte handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_bytes_view(
    bytes: *const TypeBridgeCanonicalBytes,
    out_view: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: byte handle remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[(out_view.cast(), size_of::<TypeBridgeByteView>())],
            &[(GENERATED_INPUT_CANONICAL_BYTES, bytes.cast())],
            &[],
        )
    } {
        return status;
    }
    if out_view.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller supplies one writable view slot.
    unsafe {
        out_view.write_unaligned(TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        })
    };
    guarded(|| {
        if bytes.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable bytes handle.
        let bytes = unsafe { &*bytes };
        // SAFETY: output was validated above and borrows the retained handle.
        unsafe {
            out_view.write_unaligned(TypeBridgeByteView {
                data: bytes.bytes.as_ptr(),
                length: bytes.bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the number of records in a verified archive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_count(
    archive: *const TypeBridgeCanonicalArchive,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: archive remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_count.cast(), size_of::<usize>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[(GENERATED_INPUT_CANONICAL_ARCHIVE, archive.cast())],
            &[],
        )
    } {
        return status;
    }
    if out_count.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight proved both caller outputs writable and distinct.
    unsafe {
        out_count.write_unaligned(0);
        out_diagnostics.write_unaligned(ptr::null_mut());
    }
    guarded(|| {
        if archive.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable archive.
        let archive = unsafe { &*archive };
        if let Err(diagnostic) = archive.control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        // Retain the authority carrier as part of every archive read.
        let _ = &archive.package;
        // SAFETY: output was validated above.
        unsafe { out_count.write_unaligned(archive.archive.records().len()) };
        TypeBridgeStatus::Ok
    })
}

/// Copy one indexed record from a verified archive into independent owned bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_record_at(
    archive: *const TypeBridgeCanonicalArchive,
    index: usize,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: archive remains live during complete read-only preflight.
    if let Err(status) = unsafe {
        preflight(
            &[
                (out_bytes.cast(), size_of::<*mut TypeBridgeCanonicalBytes>()),
                (
                    out_diagnostics.cast(),
                    size_of::<*mut TypeBridgeExecutionDiagnostics>(),
                ),
            ],
            &[(GENERATED_INPUT_CANONICAL_ARCHIVE, archive.cast())],
            &[],
        )
    } {
        return status;
    }
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if archive.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable archive.
        let archive = unsafe { &*archive };
        if let Err(diagnostic) = archive.control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let Some(record) = archive.archive.records().get(index) else {
            return return_execution_error(invalid_archive(), out_diagnostics);
        };
        let control = &archive.control;
        if let Err(diagnostic) = control.check() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let bytes = match encode_bytes(
            AllocationSite::CanonicalRecordEncodeBytes,
            || record.encode_with_limits(control.output_limits()),
            invalid_archive,
        ) {
            Ok(bytes) => bytes,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let bytes = match try_box(
            AllocationSite::CanonicalBytesHandle,
            TypeBridgeCanonicalBytes { bytes },
        ) {
            Ok(bytes) => bytes,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: output was validated above.
        unsafe { out_bytes.write_unaligned(Box::into_raw(bytes)) };
        TypeBridgeStatus::Ok
    })
}

/// Close an owned canonical byte buffer and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_bytes_close(
    bytes: *mut *mut TypeBridgeCanonicalBytes,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the header.
    unsafe { close_box(bytes) }
}

/// Close an archive builder and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_builder_close(
    builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the header.
    unsafe { close_box(builder) }
}

/// Close a verified archive and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_close(
    archive: *mut *mut TypeBridgeCanonicalArchive,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the header.
    unsafe { close_box(archive) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation::inject_failure;
    use crate::runtime::{
        type_bridge_cancellation_close, type_bridge_cancellation_open,
        type_bridge_cancellation_request,
    };
    use std::cell::Cell;

    #[test]
    fn canonical_input_snapshot_has_bounded_fallible_storage() {
        let input = [1_u8, 2, 3];
        let view = TypeBridgeByteView {
            data: input.as_ptr(),
            length: input.len(),
        };
        let _failure = inject_failure(AllocationSite::CanonicalInputBytes, 0);
        // SAFETY: the fixed test input remains live and readable for this call.
        let diagnostic =
            unsafe { snapshot_bytes(view, MAX_PROJECTED_RECORD_BYTES, invalid_record) }
                .expect_err("the named canonical input allocation must fail");
        assert_eq!(diagnostic.code().as_str(), "c_allocation_exhausted");
    }

    #[test]
    fn canonical_input_limit_precedes_allocation() {
        let input = [1_u8];
        let view = TypeBridgeByteView {
            data: input.as_ptr(),
            length: MAX_CANONICAL_BYTES + 1,
        };
        let _failure = inject_failure(AllocationSite::CanonicalInputBytes, 0);
        // SAFETY: the stable length ceiling is checked before the byte range is read.
        let diagnostic =
            unsafe { snapshot_bytes(view, MAX_PROJECTED_RECORD_BYTES, invalid_record) }
                .expect_err("an over-limit canonical input must fail before allocation");
        assert_eq!(diagnostic.code().as_str(), "c_canonical_input_limit");
    }

    #[test]
    fn canonical_encode_reserves_before_entering_the_codec() {
        let entered = Cell::new(false);
        let _failure = inject_failure(AllocationSite::CanonicalRecordEncodeBytes, 0);
        let diagnostic = encode_bytes(
            AllocationSite::CanonicalRecordEncodeBytes,
            || {
                entered.set(true);
                Ok(Vec::new())
            },
            invalid_record,
        )
        .expect_err("the named canonical encoding allocation must fail");
        assert!(!entered.get());
        assert_eq!(diagnostic.code().as_str(), "c_allocation_exhausted");
    }

    #[test]
    fn archive_append_and_finish_reservations_fail_without_mutation() {
        let mut builder_records = Vec::new();
        let append_failure = inject_failure(AllocationSite::CanonicalArchiveBuilderRecords, 0);
        assert!(reserve_archive_record(&mut builder_records).is_err());
        assert!(builder_records.is_empty());
        drop(append_failure);

        let finish_failure = inject_failure(AllocationSite::CanonicalArchiveFinishRecords, 0);
        assert!(clone_archive_records(&builder_records).is_err());
        assert!(builder_records.is_empty());
        drop(finish_failure);

        assert!(clone_archive_records(&builder_records).is_ok());
    }

    #[test]
    fn codec_options_are_exact_tighten_only_and_retain_cancellation_state() {
        assert_eq!(size_of::<TypeBridgeProjectedCodecOptionsV1>(), 72);
        let mut cancellation = ptr::null_mut();
        // SAFETY: the owner slot is writable and initially null.
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        let options = TypeBridgeProjectedCodecOptionsV1 {
            struct_size: size_of::<TypeBridgeProjectedCodecOptionsV1>() as u64,
            version: PROJECTED_CODEC_OPTIONS_V1,
            flags: 0,
            timeout_milliseconds: 0,
            max_input_bytes: 17,
            max_output_bytes: 19,
            max_depth: 3,
            max_records: 2,
            max_members: 5,
            cancellation,
        };
        // SAFETY: the complete options and cancellation handles remain live for capture.
        let control = unsafe { canonical_control(&options, true) }.expect("options are valid");
        assert_eq!(control.max_input_bytes, 17);
        assert_eq!(control.max_output_bytes, 19);
        assert_eq!(control.max_depth, 3);
        assert_eq!(control.max_records, 2);
        assert_eq!(control.max_members, 5);
        assert!(control.check().is_ok());

        // SAFETY: the live cancellation handle accepts one sticky request.
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: the exact owner slot is closed and cleared after control capture.
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            control
                .check()
                .expect_err("captured cancellation is sticky")
                .code()
                .as_str(),
            "provider_cancelled",
        );
    }

    #[test]
    fn codec_options_reject_layout_flags_and_expired_deadlines() {
        let mut options = TypeBridgeProjectedCodecOptionsV1 {
            struct_size: 0,
            version: PROJECTED_CODEC_OPTIONS_V1,
            flags: 0,
            timeout_milliseconds: 0,
            max_input_bytes: 1,
            max_output_bytes: 1,
            max_depth: 1,
            max_records: 1,
            max_members: 1,
            cancellation: ptr::null(),
        };
        // SAFETY: the complete deliberately invalid options object remains readable.
        assert!(matches!(
            unsafe { canonical_control(&options, false) },
            Err(TypeBridgeStatus::InvalidArgument)
        ));
        options.struct_size = size_of::<TypeBridgeProjectedCodecOptionsV1>() as u64;
        options.flags = 2;
        // SAFETY: the complete deliberately invalid options object remains readable.
        assert!(matches!(
            unsafe { canonical_control(&options, false) },
            Err(TypeBridgeStatus::InvalidArgument)
        ));
        options.flags = PROJECTED_CODEC_HAS_TIMEOUT;
        // SAFETY: the complete options object remains readable during deadline capture.
        let control = unsafe { canonical_control(&options, false) }.expect("layout is valid");
        assert_eq!(
            control
                .check()
                .expect_err("zero timeout expires immediately")
                .code()
                .as_str(),
            "transaction_deadline_exceeded",
        );
    }
}
