//! Private ABI 1.6 owned canonical bytes and deterministic archive handles.

use std::ptr;
use std::sync::Arc;

use type_bridge_contract::limits::MAX_CANONICAL_BYTES;
use type_bridge_contract::projected_record::{ProjectedArchive, ProjectedRecord};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, close_box,
    guarded,
};
use crate::allocation::{AllocationSite, allocation_exhausted, try_box};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedReference, TypeBridgeProjectedThing,
};
use crate::projected_value::TypeBridgeProjectedValue;

/// Opaque owned immutable canonical byte buffer.
pub struct TypeBridgeCanonicalBytes {
    bytes: Vec<u8>,
}

/// Opaque bounded deterministic archive builder.
pub struct TypeBridgeCanonicalArchiveBuilder {
    package: Arc<SchemaPackageState>,
    records: Vec<ProjectedRecord>,
}

/// Opaque verified immutable canonical archive.
pub struct TypeBridgeCanonicalArchive {
    package: Arc<SchemaPackageState>,
    archive: ProjectedArchive,
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

fn invalid_value() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_canonical_value_invalid"),
        message("Projected value cannot be encoded by this installed schema package"),
    )
}

fn publish_record(
    record: Result<ProjectedRecord, type_bridge_orm::ProjectedCodecError>,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let record = match record {
        Ok(record) => record,
        Err(_) => return return_execution_error(invalid_value(), out_diagnostics),
    };
    let bytes = match record.encode() {
        Ok(bytes) => bytes,
        Err(_) => return return_execution_error(invalid_value(), out_diagnostics),
    };
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

/// Encode one exact package-branded projected attribute value as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_attribute_v1(
    value: *const TypeBridgeProjectedValue,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
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
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded projected create as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_create_v1(
    value: *const TypeBridgeProjectedCreate,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains one immutable projected create for this call.
        let value = unsafe { &*value };
        publish_record(
            type_bridge_orm::record_from_create(&value.package.installed_projection, &value.value),
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded detached reference as canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_reference_v1(
    value: *const TypeBridgeProjectedReference,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
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
            out_bytes,
            out_diagnostics,
        )
    })
}

/// Encode one exact package-branded hydrated thing as a detached canonical snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_record_encode_snapshot_v1(
    value: *const TypeBridgeProjectedThing,
    out_bytes: *mut *mut TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
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
            out_bytes,
            out_diagnostics,
        )
    })
}

unsafe fn snapshot_bytes(view: TypeBridgeByteView) -> Result<Vec<u8>, TypeBridgeStatus> {
    if view.data.is_null() || view.length == 0 {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if view.length > MAX_CANONICAL_BYTES {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    // SAFETY: the C contract requires the non-null input range to remain readable
    // for this call; u8 has alignment one and the stable ceiling bounds the copy.
    Ok(unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec())
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

/// Open an empty archive builder bound to one exact installed package.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_canonical_archive_builder_open_v1(
    package: *const TypeBridgeSchemaPackage,
    out_builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_builder, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller retains a live immutable package for this call.
        let package = unsafe { &*package };
        let builder = TypeBridgeCanonicalArchiveBuilder {
            package: Arc::clone(package.state()),
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
        let bytes = match unsafe { snapshot_bytes(record) } {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        let record = match validate_record(&builder.package, &bytes) {
            Ok(record) => record,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if builder.records.try_reserve(1).is_err() {
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
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_bytes, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplies a live owning builder slot.
        let retained = unsafe { builder.read_unaligned() };
        if retained.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the retained builder remains owned by the caller until success.
        let value = unsafe { &*retained };
        let archive = match ProjectedArchive::try_new(value.records.clone()) {
            Ok(archive) => archive,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
        };
        let bytes = match archive.encode() {
            Ok(bytes) => bytes,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
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
    out_archive: *mut *mut TypeBridgeCanonicalArchive,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared initializer validates and clears distinct writable outputs.
    if let Err(status) = unsafe { initialize_execution_outputs(out_archive, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both input objects for this call.
        let package = unsafe { &*package };
        let bytes = match unsafe { snapshot_bytes(bytes) } {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        let archive = match ProjectedArchive::decode(&bytes) {
            Ok(archive) => archive,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
        };
        for record in archive.records() {
            if type_bridge_orm::materialize_record(&package.state().installed_projection, record)
                .is_err()
            {
                return return_execution_error(invalid_archive(), out_diagnostics);
            }
        }
        let archive = TypeBridgeCanonicalArchive {
            package: Arc::clone(package.state()),
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
) -> TypeBridgeStatus {
    if out_count.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller supplies one writable count slot.
    unsafe { out_count.write_unaligned(0) };
    guarded(|| {
        if archive.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable archive.
        let archive = unsafe { &*archive };
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
        let Some(record) = archive.archive.records().get(index) else {
            return return_execution_error(invalid_archive(), out_diagnostics);
        };
        let bytes = match record.encode() {
            Ok(bytes) => bytes,
            Err(_) => return return_execution_error(invalid_archive(), out_diagnostics),
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
