use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{DiagnosticCategory, DiagnosticPathSegment};
use type_bridge_contract::limits::{
    MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::projection::{
    RuntimeProjection, TYPE_BRIDGE_C_ABI_MAJOR, TYPE_BRIDGE_C_ABI_MINOR,
};
use type_bridge_schema::VerifiedSchemaAuthority;

use crate::diagnostic::{diagnostics_handle, stable};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::schema_package;

pub(crate) const ABI_MAJOR: u32 = TYPE_BRIDGE_C_ABI_MAJOR;
pub(crate) const ABI_MINOR: u32 = TYPE_BRIDGE_C_ABI_MINOR;
pub(crate) const SCHEMA_PACKAGE_CHUNK_BYTES_MAX: usize = 32_768;
pub(crate) const SCHEMA_PACKAGE_HOSTED_OBJECT_BYTES_MAX: usize = 65_535;
const RUNTIME_VERSION: &[u8] = env!("CARGO_PKG_VERSION").as_bytes();

/// Frozen layout version for a chunked byte view.
pub const TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION: u32 = 1;

/// Stable integer result returned by every fallible C ABI function.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeStatus {
    /// The operation completed successfully.
    Ok = 0,
    /// A required pointer, output slot, or pointer/length pair was invalid.
    InvalidArgument = 1,
    /// Canonical schema-package evidence was rejected.
    SchemaPackageRejected = 2,
    /// The package requires an unsupported version or capability.
    Unsupported = 3,
    /// A bounded input exceeded the stable resource ceiling.
    ResourceLimit = 4,
    /// Runtime execution failed; typed diagnostics carry the semantic cause.
    ExecutionFailed = 5,
    /// Execution was cancelled cooperatively before or during supported work.
    Cancelled = 6,
    /// A failed commit has an unknown durability outcome.
    CommitOutcomeUnknown = 7,
    /// A resource remains in use by an operation that prevents closing it.
    InUse = 8,
    /// A Rust panic was contained at the ABI boundary.
    Panic = 255,
}

/// Borrowed immutable bytes crossing the C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeByteView {
    /// First byte, or null only when `length` is zero.
    pub data: *const u8,
    /// Number of readable bytes.
    pub length: usize,
}

impl TypeBridgeByteView {
    const EMPTY: Self = Self {
        data: ptr::null(),
        length: 0,
    };

    fn from_slice(value: &[u8]) -> Self {
        Self {
            data: value.as_ptr(),
            length: value.len(),
        }
    }

    pub(crate) unsafe fn snapshot(
        self,
        field: &'static str,
        maximum: usize,
    ) -> Result<
        Vec<u8>,
        (
            TypeBridgeStatus,
            Vec<type_bridge_contract::diagnostic::Diagnostic>,
        ),
    > {
        if self.length == 0 || self.data.is_null() {
            return Err(invalid_view(
                field,
                "C schema descriptor byte views must be non-empty and non-null",
            ));
        }
        if self.length > maximum {
            let diagnostic = stable(
                DiagnosticCategory::ResourceLimit,
                "c_schema_descriptor_byte_limit_exceeded",
                "C schema descriptor byte view exceeds its stable limit",
            )
            .at(DiagnosticPathSegment::Field(field.to_owned()));
            return Err((TypeBridgeStatus::ResourceLimit, vec![diagnostic]));
        }
        // SAFETY: the caller contract requires `data..data+length` to be readable
        // and immutable for this call. Null and length bounds were checked above;
        // u8 has alignment one. Snapshotting ends caller ownership immediately.
        Ok(unsafe { std::slice::from_raw_parts(self.data, self.length) }.to_vec())
    }
}

pub(crate) fn invalid_view(
    field: &'static str,
    message: &'static str,
) -> (
    TypeBridgeStatus,
    Vec<type_bridge_contract::diagnostic::Diagnostic>,
) {
    (
        TypeBridgeStatus::InvalidArgument,
        vec![
            stable(
                DiagnosticCategory::InvalidContract,
                "c_schema_descriptor_invalid_view",
                message,
            )
            .at(DiagnosticPathSegment::Field(field.to_owned())),
        ],
    )
}

/// Version-1 generated schema-package descriptor.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeSchemaPackageDescriptorV1 {
    /// Exact `sizeof` of this descriptor in the generated source.
    pub struct_size: u32,
    /// Required ABI major.
    pub abi_major: u32,
    /// Minimum ABI minor required by the generated package.
    pub abi_minor: u32,
    /// Canonical schema-authority bytes.
    pub schema_authority_json: TypeBridgeByteView,
    /// Exact canonical declared-schema bytes duplicated for verification.
    pub declared_schema_json: TypeBridgeByteView,
    /// Canonical target RuntimeProjection bytes.
    pub runtime_projection_json: TypeBridgeByteView,
    /// Detached canonical semantic fingerprint bytes.
    pub semantic_fingerprint_json: TypeBridgeByteView,
    /// Detached canonical binding fingerprint bytes.
    pub binding_fingerprint_json: TypeBridgeByteView,
    /// Exact managed-scope ID bytes.
    pub managed_scope: TypeBridgeByteView,
    /// Exact semantic-profile ID bytes.
    pub semantic_profile: TypeBridgeByteView,
    /// Reserved zero words for compatible descriptor growth.
    pub reserved: [u64; 4],
}

/// Version-1 bounded scatter view used by generated large schema packages.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeChunkedByteViewV1 {
    /// Exact size of this view descriptor.
    pub struct_size: u32,
    /// Frozen chunked byte-view layout version.
    pub version: u32,
    /// Readable table of immutable nonempty byte chunks.
    pub chunks: *const TypeBridgeByteView,
    /// Number of table elements.
    pub chunk_count: usize,
    /// Exact sum of every chunk length.
    pub total_length: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 generated schema-package descriptor using bounded byte chunks.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeSchemaPackageChunkedDescriptorV1 {
    /// Exact `sizeof` of this descriptor in generated source.
    pub struct_size: u32,
    /// Required ABI major.
    pub abi_major: u32,
    /// Minimum ABI minor required by the generated package.
    pub abi_minor: u32,
    /// Reserved zero word for exact layout stability.
    pub reserved0: u32,
    /// Chunked canonical schema-authority bytes.
    pub schema_authority_json: TypeBridgeChunkedByteViewV1,
    /// Chunked exact canonical declared-schema bytes.
    pub declared_schema_json: TypeBridgeChunkedByteViewV1,
    /// Chunked canonical target RuntimeProjection bytes.
    pub runtime_projection_json: TypeBridgeChunkedByteViewV1,
    /// Chunked detached canonical semantic fingerprint bytes.
    pub semantic_fingerprint_json: TypeBridgeChunkedByteViewV1,
    /// Chunked detached canonical binding fingerprint bytes.
    pub binding_fingerprint_json: TypeBridgeChunkedByteViewV1,
    /// Chunked exact managed-scope ID bytes.
    pub managed_scope: TypeBridgeChunkedByteViewV1,
    /// Chunked exact semantic-profile ID bytes.
    pub semantic_profile: TypeBridgeChunkedByteViewV1,
    /// Reserved zero words for compatible descriptor growth.
    pub reserved: [u64; 4],
}

/// Opaque verified schema-package handle owned by the Rust runtime.
pub struct TypeBridgeSchemaPackage {
    pub(crate) state: Arc<SchemaPackageState>,
}

/// Immutable verified package state retained by projected handles.
pub(crate) struct SchemaPackageState {
    pub(crate) authority_json: Vec<u8>,
    pub(crate) projection_json: Vec<u8>,
    pub(crate) semantic_fingerprint_json: Vec<u8>,
    pub(crate) binding_fingerprint_json: Vec<u8>,
    pub(crate) managed_scope: Vec<u8>,
    pub(crate) semantic_profile: Vec<u8>,
    pub(crate) _authority: VerifiedSchemaAuthority,
    pub(crate) _projection: RuntimeProjection,
    pub(crate) installed_projection: type_bridge_orm::InstalledRuntimeProjection,
}

impl TypeBridgeSchemaPackage {
    pub(crate) fn state(&self) -> &Arc<SchemaPackageState> {
        &self.state
    }
}

/// Opaque ordered structured-diagnostic handle owned by the Rust runtime.
pub struct TypeBridgeDiagnostics {
    pub(crate) encoded: Vec<u8>,
}

pub(crate) fn guarded(operation: impl FnOnce() -> TypeBridgeStatus) -> TypeBridgeStatus {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(TypeBridgeStatus::Panic)
}

pub(crate) unsafe fn initialize_view(out: *mut TypeBridgeByteView) -> Result<(), TypeBridgeStatus> {
    if out.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: the caller supplied a non-null writable output slot.
    unsafe { out.write(TypeBridgeByteView::EMPTY) };
    Ok(())
}

pub(crate) fn borrowed_view(
    value: &[u8],
    out: *mut TypeBridgeByteView,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: delegated C API validation initializes the caller's output slot.
    unsafe { initialize_view(out)? };
    // SAFETY: `out` was validated writable above and the returned bytes remain
    // owned by a live opaque handle or the static runtime version.
    unsafe { out.write(TypeBridgeByteView::from_slice(value)) };
    Ok(())
}

/// Return the supported C ABI major version.
#[unsafe(no_mangle)]
pub extern "C" fn type_bridge_c_abi_major() -> u32 {
    catch_unwind(|| ABI_MAJOR).unwrap_or(0)
}

/// Return the supported C ABI minor version.
#[unsafe(no_mangle)]
pub extern "C" fn type_bridge_c_abi_minor() -> u32 {
    catch_unwind(|| ABI_MINOR).unwrap_or(0)
}

/// Borrow the TypeBridge semantic package version as non-NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_runtime_version(
    out_version: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| match borrowed_view(RUNTIME_VERSION, out_version) {
        Ok(()) => TypeBridgeStatus::Ok,
        Err(status) => status,
    })
}

fn package_open_output_preflight(
    out_package: *mut *mut TypeBridgeSchemaPackage,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[
        (
            out_package.cast(),
            size_of::<*mut TypeBridgeSchemaPackage>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeDiagnostics>(),
        ),
    ])
}

unsafe fn initialize_package_open_outputs(
    out_package: *mut *mut TypeBridgeSchemaPackage,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if !out_package.is_null() {
        // SAFETY: preflight proved this caller slot writable and disjoint.
        unsafe { out_package.write_unaligned(ptr::null_mut()) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: preflight proved this caller slot writable and disjoint.
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_package.is_null() || out_diagnostics.is_null() {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

fn legacy_descriptor_layout_is_consumed(descriptor: &TypeBridgeSchemaPackageDescriptorV1) -> bool {
    descriptor.struct_size as usize == size_of::<TypeBridgeSchemaPackageDescriptorV1>()
        && descriptor.reserved == [0; 4]
        && descriptor.abi_major == ABI_MAJOR
        && descriptor.abi_minor <= ABI_MINOR
}

fn check_consumed_byte_view(
    preflight: &DirectOutputPreflight,
    view: TypeBridgeByteView,
    maximum: usize,
) -> Result<(), TypeBridgeStatus> {
    if view.length != 0 && view.length <= maximum && !view.data.is_null() {
        preflight.check_bytes(view.data.cast(), view.length)?;
    }
    Ok(())
}

unsafe fn preflight_legacy_package_descriptor(
    preflight: &DirectOutputPreflight,
    descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
) -> Result<(), TypeBridgeStatus> {
    if descriptor.is_null() {
        return Ok(());
    }
    preflight.check_bytes(
        descriptor.cast(),
        size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
    )?;
    // SAFETY: the complete top-level descriptor was checked before this snapshot.
    let descriptor = unsafe { descriptor.read_unaligned() };
    if !legacy_descriptor_layout_is_consumed(&descriptor) {
        return Ok(());
    }
    for (view, maximum) in [
        (descriptor.schema_authority_json, MAX_CANONICAL_BYTES),
        (descriptor.declared_schema_json, MAX_CANONICAL_BYTES),
        (descriptor.runtime_projection_json, MAX_CANONICAL_BYTES),
        (descriptor.semantic_fingerprint_json, MAX_CANONICAL_BYTES),
        (descriptor.binding_fingerprint_json, MAX_CANONICAL_BYTES),
        (descriptor.managed_scope, MAX_CANONICAL_STRING_BYTES),
        (descriptor.semantic_profile, MAX_CANONICAL_STRING_BYTES),
    ] {
        check_consumed_byte_view(preflight, view, maximum)?;
    }
    Ok(())
}

fn chunked_descriptor_layout_is_consumed(
    descriptor: &TypeBridgeSchemaPackageChunkedDescriptorV1,
) -> bool {
    descriptor.struct_size as usize == size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>()
        && descriptor.reserved0 == 0
        && descriptor.reserved == [0; 4]
        && descriptor.abi_major == ABI_MAJOR
        && descriptor.abi_minor <= ABI_MINOR
}

fn chunked_view_layout_is_consumed(view: &TypeBridgeChunkedByteViewV1, maximum: usize) -> bool {
    view.struct_size as usize == size_of::<TypeBridgeChunkedByteViewV1>()
        && view.version == TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION
        && view.reserved == [0; 4]
        && view.chunk_count != 0
        && view.chunk_count <= MAX_CANONICAL_COLLECTION_LEN
        && !view.chunks.is_null()
        && view.total_length != 0
        && view.total_length <= maximum
}

unsafe fn preflight_chunked_view(
    preflight: &DirectOutputPreflight,
    view: TypeBridgeChunkedByteViewV1,
    maximum: usize,
) -> Result<(), TypeBridgeStatus> {
    if !chunked_view_layout_is_consumed(&view, maximum) {
        return Ok(());
    }
    let table_bytes = view
        .chunk_count
        .checked_mul(size_of::<TypeBridgeByteView>())
        .filter(|bytes| *bytes <= SCHEMA_PACKAGE_HOSTED_OBJECT_BYTES_MAX)
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    preflight.check_bytes(view.chunks.cast(), table_bytes)?;
    let mut total = 0_usize;
    for index in 0..view.chunk_count {
        // SAFETY: the complete bounded table storage was checked above.
        let chunk = unsafe { view.chunks.add(index).read_unaligned() };
        if chunk.length == 0
            || chunk.length > SCHEMA_PACKAGE_CHUNK_BYTES_MAX
            || chunk.data.is_null()
        {
            break;
        }
        let Some(next) = total.checked_add(chunk.length) else {
            break;
        };
        if next > view.total_length || next > maximum {
            break;
        }
        preflight.check_bytes(chunk.data.cast(), chunk.length)?;
        total = next;
    }
    Ok(())
}

unsafe fn preflight_chunked_package_descriptor(
    preflight: &DirectOutputPreflight,
    descriptor: *const TypeBridgeSchemaPackageChunkedDescriptorV1,
) -> Result<(), TypeBridgeStatus> {
    if descriptor.is_null() {
        return Ok(());
    }
    preflight.check_bytes(
        descriptor.cast(),
        size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
    )?;
    // SAFETY: the complete top-level descriptor was checked before this snapshot.
    let descriptor = unsafe { descriptor.read_unaligned() };
    if !chunked_descriptor_layout_is_consumed(&descriptor) {
        return Ok(());
    }
    let views = [
        (descriptor.schema_authority_json, MAX_CANONICAL_BYTES),
        (descriptor.declared_schema_json, MAX_CANONICAL_BYTES),
        (descriptor.runtime_projection_json, MAX_CANONICAL_BYTES),
        (descriptor.semantic_fingerprint_json, MAX_CANONICAL_BYTES),
        (descriptor.binding_fingerprint_json, MAX_CANONICAL_BYTES),
        (descriptor.managed_scope, MAX_CANONICAL_STRING_BYTES),
        (descriptor.semantic_profile, MAX_CANONICAL_STRING_BYTES),
    ];
    let aggregate = views.iter().try_fold(0_usize, |total, (view, _)| {
        total.checked_add(view.chunk_count)
    });
    if aggregate.is_none_or(|count| count > MAX_CANONICAL_COLLECTION_LEN) {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    for (view, maximum) in views {
        // SAFETY: the aggregate pass bounded all table walks before this call.
        unsafe { preflight_chunked_view(preflight, view, maximum) }?;
    }
    Ok(())
}

/// Snapshot and verify a generated version-1 schema-package descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_schema_package_open_v1(
    descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
    out_package: *mut *mut TypeBridgeSchemaPackage,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match package_open_output_preflight(out_package, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: this performs only read-only exact-range checks before caller writes.
    if let Err(status) = unsafe { preflight_legacy_package_descriptor(&preflight, descriptor) } {
        return status;
    }
    // SAFETY: outputs are pairwise disjoint and disjoint from every consumed input range.
    if let Err(status) = unsafe { initialize_package_open_outputs(out_package, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if descriptor.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: unaligned reads admit hostile-but-readable descriptor storage;
        // nested byte views are validated and snapshotted before decoding.
        let descriptor = unsafe { descriptor.read_unaligned() };
        match schema_package::open(descriptor) {
            Ok(package) => {
                // SAFETY: output was initialized and remains caller-writable.
                unsafe { out_package.write_unaligned(Box::into_raw(Box::new(package))) };
                TypeBridgeStatus::Ok
            }
            Err((status, diagnostics)) => {
                // SAFETY: output was initialized and remains caller-writable.
                unsafe {
                    out_diagnostics
                        .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
                };
                status
            }
        }
    })
}

/// Snapshot, reassemble, and verify a generated chunked schema-package descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_schema_package_open_chunked_v1(
    descriptor: *const TypeBridgeSchemaPackageChunkedDescriptorV1,
    out_package: *mut *mut TypeBridgeSchemaPackage,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match package_open_output_preflight(out_package, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: this performs only read-only bounded graph checks before caller writes.
    if let Err(status) = unsafe { preflight_chunked_package_descriptor(&preflight, descriptor) } {
        return status;
    }
    // SAFETY: outputs are pairwise disjoint and disjoint from every consumed input range.
    if let Err(status) = unsafe { initialize_package_open_outputs(out_package, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if descriptor.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: preflight checked the complete top-level descriptor range.
        let descriptor = unsafe { descriptor.read_unaligned() };
        // SAFETY: preflight checked every table/data range that valid layout can consume.
        match unsafe { schema_package::open_chunked(descriptor) } {
            Ok(package) => {
                // SAFETY: output was initialized and remains caller-writable.
                unsafe { out_package.write_unaligned(Box::into_raw(Box::new(package))) };
                TypeBridgeStatus::Ok
            }
            Err((status, diagnostics)) => {
                // SAFETY: output was initialized and remains caller-writable.
                unsafe {
                    out_diagnostics
                        .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
                };
                status
            }
        }
    })
}

pub(crate) unsafe fn close_box<T>(slot: *mut *mut T) -> TypeBridgeStatus {
    guarded(|| {
        if slot.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller supplies a writable slot containing either null or
        // one pointer previously returned by this exact TypeBridge close family.
        let value = unsafe { slot.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // Clear first so ordinary repeated close through the same slot is safe.
        unsafe { slot.write(ptr::null_mut()) };
        // SAFETY: ownership of this exact allocation returns to Rust once.
        unsafe { drop(Box::from_raw(value)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one Rust-owned schema package and clear the caller's slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_schema_package_close(
    package: *mut *mut TypeBridgeSchemaPackage,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(package) }
}

macro_rules! package_view {
    ($function:ident, $field:ident, $doc:literal) => {
        #[doc = $doc]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            package: *const TypeBridgeSchemaPackage,
            out_value: *mut TypeBridgeByteView,
        ) -> TypeBridgeStatus {
            guarded(|| {
                // SAFETY: initialize before inspecting the input handle.
                if let Err(status) = unsafe { initialize_view(out_value) } {
                    return status;
                }
                if package.is_null() {
                    return TypeBridgeStatus::InvalidArgument;
                }
                // SAFETY: caller retains a live immutable package handle for this call.
                let package = unsafe { &*package };
                match borrowed_view(&package.state().$field, out_value) {
                    Ok(()) => TypeBridgeStatus::Ok,
                    Err(status) => status,
                }
            })
        }
    };
}

package_view!(
    type_bridge_schema_package_authority_json,
    authority_json,
    "Borrow exact canonical schema-authority bytes from a live package."
);
package_view!(
    type_bridge_schema_package_projection_json,
    projection_json,
    "Borrow exact canonical runtime-projection bytes from a live package."
);
package_view!(
    type_bridge_schema_package_semantic_fingerprint_json,
    semantic_fingerprint_json,
    "Borrow exact canonical semantic-fingerprint bytes from a live package."
);
package_view!(
    type_bridge_schema_package_binding_fingerprint_json,
    binding_fingerprint_json,
    "Borrow exact canonical binding-fingerprint bytes from a live package."
);
package_view!(
    type_bridge_schema_package_managed_scope,
    managed_scope,
    "Borrow exact managed-scope ID bytes from a live package."
);
package_view!(
    type_bridge_schema_package_semantic_profile,
    semantic_profile,
    "Borrow exact semantic-profile ID bytes from a live package."
);

/// Borrow canonical structured diagnostic JSON from a live diagnostic handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_diagnostics_json(
    diagnostics: *const TypeBridgeDiagnostics,
    out_json: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| {
        // SAFETY: initialize before inspecting the input handle.
        if let Err(status) = unsafe { initialize_view(out_json) } {
            return status;
        }
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable diagnostics handle for this call.
        let diagnostics = unsafe { &*diagnostics };
        match borrowed_view(&diagnostics.encoded, out_json) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Close one Rust-owned diagnostic handle and clear the caller's slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_diagnostics_close(
    diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(diagnostics) }
}

#[cfg(test)]
mod tests {
    use super::{TypeBridgeStatus, guarded};

    #[test]
    fn panic_guard_maps_unwinding_to_the_stable_status() {
        assert_eq!(
            guarded(|| panic!("intentional C ABI panic-containment probe")),
            TypeBridgeStatus::Panic,
        );
    }
}
