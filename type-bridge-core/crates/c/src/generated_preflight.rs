//! Read-only alias preflight used by emitted nominal C wrappers.

use std::ffi::c_void;
use std::mem::size_of;

use type_bridge_contract::limits::MAX_CANONICAL_COLLECTION_LEN;
use type_bridge_orm::ProjectedResourceMeasure;

use crate::abi::{
    SchemaPackageState, TypeBridgeDiagnostics, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded,
};
use crate::canonical_archive::{
    TypeBridgeCanonicalArchive, TypeBridgeCanonicalArchiveBuilder, TypeBridgeCanonicalBytes,
    TypeBridgeProjectedCodecOptionsV1, TypeBridgeProjectedStruct, TypeBridgeProjectedStructMember,
};
use crate::execution_diagnostic::TypeBridgeExecutionDiagnostics;
use crate::projected_batch::{
    TypeBridgeProjectedBatch, TypeBridgeProjectedBatchBuilder, TypeBridgeProjectedBatchResult,
};
use crate::projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedReference, TypeBridgeProjectedThing,
};
use crate::projected_token::TypeBridgeProjectedTokenV1;
use crate::projected_value::TypeBridgeProjectedValue;
use crate::query::{
    TypeBridgeQuery, TypeBridgeQueryBinding, TypeBridgeQueryField, TypeBridgeQueryFunction,
    TypeBridgeQueryFunctionCall, TypeBridgeQueryFunctionValue, TypeBridgeQueryOrder,
    TypeBridgeQueryPredicate, TypeBridgeQueryRemoteClaim, TypeBridgeQueryRemoteContext,
    TypeBridgeQueryRemotePending, TypeBridgeQueryResult, TypeBridgeQueryRole,
    TypeBridgeQuerySelection, TypeBridgeQuerySession, TypeBridgeQueryTerminal,
};
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction, TypeBridgeRuntime,
    TypeBridgeWriteTransaction,
};

const GENERATED_PREFLIGHT_VERSION: u32 = 1;
const GENERATED_PREFLIGHT_OUTPUT_COUNT_MAX: usize = 2;
const DIRECT_OUTPUT_COUNT_MAX: usize = 3;

/// Raw byte-range input kind.
pub const GENERATED_INPUT_BYTES: u32 = 1;
/// Generated projected-token input kind.
pub const GENERATED_INPUT_PROJECTED_TOKEN: u32 = 2;
/// Verified schema-package handle input kind.
pub const GENERATED_INPUT_SCHEMA_PACKAGE: u32 = 3;
/// Runtime handle input kind.
pub const GENERATED_INPUT_RUNTIME: u32 = 4;
/// Database handle input kind.
pub const GENERATED_INPUT_DATABASE: u32 = 5;
/// Read-transaction handle input kind.
pub const GENERATED_INPUT_READ_TRANSACTION: u32 = 6;
/// Write-transaction handle input kind.
pub const GENERATED_INPUT_WRITE_TRANSACTION: u32 = 7;
/// Cancellation handle input kind.
pub const GENERATED_INPUT_CANCELLATION: u32 = 8;
/// Projected scalar-value handle input kind.
pub const GENERATED_INPUT_PROJECTED_VALUE: u32 = 9;
/// Projected reference handle input kind.
pub const GENERATED_INPUT_PROJECTED_REFERENCE: u32 = 10;
/// Projected create handle input kind.
pub const GENERATED_INPUT_PROJECTED_CREATE: u32 = 11;
/// Projected hydrated-thing handle input kind.
pub const GENERATED_INPUT_PROJECTED_THING: u32 = 12;
/// Pointer array of projected scalar-value handles input kind.
pub const GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY: u32 = 13;
/// Pointer array of projected reference handles input kind.
pub const GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY: u32 = 14;
/// Schema-package diagnostic handle input kind.
pub const GENERATED_INPUT_DIAGNOSTICS: u32 = 15;
/// Execution-diagnostics handle input kind.
pub const GENERATED_INPUT_EXECUTION_DIAGNOSTICS: u32 = 16;
/// Complete generated nominal create-argument graph input kind.
pub const GENERATED_INPUT_CREATE_ARGS_GRAPH: u32 = 17;
/// Typed-query construction-session handle input kind.
pub const GENERATED_INPUT_QUERY_SESSION: u32 = 18;
/// Typed-query binding handle input kind.
pub const GENERATED_INPUT_QUERY_BINDING: u32 = 19;
/// Typed-query field handle input kind.
pub const GENERATED_INPUT_QUERY_FIELD: u32 = 20;
/// Typed-query role handle input kind.
pub const GENERATED_INPUT_QUERY_ROLE: u32 = 21;
/// Typed-query predicate handle input kind.
pub const GENERATED_INPUT_QUERY_PREDICATE: u32 = 22;
/// Typed-query order handle input kind.
pub const GENERATED_INPUT_QUERY_ORDER: u32 = 23;
/// Typed-query selection handle input kind.
pub const GENERATED_INPUT_QUERY_SELECTION: u32 = 24;
/// Typed-query lineage handle input kind.
pub const GENERATED_INPUT_QUERY: u32 = 25;
/// Typed-query terminal handle input kind.
pub const GENERATED_INPUT_QUERY_TERMINAL: u32 = 26;
/// Typed-query materialized-result handle input kind.
pub const GENERATED_INPUT_QUERY_RESULT: u32 = 27;
/// Caller-transport remote query context input kind.
pub const GENERATED_INPUT_QUERY_REMOTE_CONTEXT: u32 = 28;
/// Prepared remote query request input kind.
pub const GENERATED_INPUT_QUERY_REMOTE_PENDING: u32 = 29;
/// One-shot remote reply claim input kind.
pub const GENERATED_INPUT_QUERY_REMOTE_CLAIM: u32 = 30;
/// Exact generated schema-function handle input kind.
pub const GENERATED_INPUT_QUERY_FUNCTION: u32 = 31;
/// Session-branded schema-function scalar-value input kind.
pub const GENERATED_INPUT_QUERY_FUNCTION_VALUE: u32 = 32;
/// Immutable scalar schema-function call input kind.
pub const GENERATED_INPUT_QUERY_FUNCTION_CALL: u32 = 33;
/// Projected-batch construction builder handle input kind.
pub const GENERATED_INPUT_PROJECTED_BATCH_BUILDER: u32 = 34;
/// Immutable reusable projected-batch handle input kind.
pub const GENERATED_INPUT_PROJECTED_BATCH: u32 = 35;
/// Immutable projected-batch result handle input kind.
pub const GENERATED_INPUT_PROJECTED_BATCH_RESULT: u32 = 36;
/// Owned canonical-byte handle input kind.
pub const GENERATED_INPUT_CANONICAL_BYTES: u32 = 37;
/// Canonical archive-builder handle input kind.
pub const GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER: u32 = 38;
/// Verified canonical archive handle input kind.
pub const GENERATED_INPUT_CANONICAL_ARCHIVE: u32 = 39;
/// Decoded projected-struct handle input kind.
pub const GENERATED_INPUT_PROJECTED_STRUCT: u32 = 40;
/// Independently owned projected-struct member input kind.
pub const GENERATED_INPUT_PROJECTED_STRUCT_MEMBER: u32 = 41;
/// Versioned canonical codec options input kind.
pub const GENERATED_INPUT_PROJECTED_CODEC_OPTIONS: u32 = 42;

/// Frozen generated create-argument graph layout version.
pub const GENERATED_CREATE_GRAPH_VERSION: u32 = 1;
/// Maximum number of generated create member descriptors.
pub const GENERATED_CREATE_MEMBER_COUNT_MAX: usize = 1_020;
/// Minimum C hosted-object ceiling used for every traversed object.
pub const GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX: usize = 65_535;

/// Generated scalar-value member kind.
pub const GENERATED_CREATE_MEMBER_VALUE_SCALAR: u32 = 1;
/// Generated sequence-value member kind.
pub const GENERATED_CREATE_MEMBER_VALUE_SEQUENCE: u32 = 2;
/// Generated scalar-reference member kind.
pub const GENERATED_CREATE_MEMBER_REFERENCE_SCALAR: u32 = 3;
/// Generated sequence-reference member kind.
pub const GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE: u32 = 4;

/// Version-1 generic view of one linked nominal create handle chunk.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeGeneratedCreateHandleChunkV1 {
    /// Exact size of the nominal chunk object.
    pub struct_size: usize,
    /// Frozen generated create-graph layout version.
    pub version: u32,
    /// Immutable pointer array of projected value or reference handles.
    pub values: *const *const c_void,
    /// Number of handles in this nonempty chunk.
    pub count: usize,
    /// Next immutable chunk, or null at the end.
    pub next: *const TypeBridgeGeneratedCreateHandleChunkV1,
    /// Reserved zero words for compatible growth.
    pub reserved: [u32; 4],
}

/// Version-1 static metadata for one generated nominal create member.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeGeneratedCreateMemberV1 {
    /// Exact size of this member descriptor.
    pub struct_size: u32,
    /// Frozen generated create-graph layout version.
    pub version: u32,
    /// One closed `TYPE_BRIDGE_GENERATED_CREATE_MEMBER_*` kind.
    pub kind: u32,
    /// Explicit padding; must be zero.
    pub reserved0: u32,
    /// Byte offset of this member pointer within the nominal args object.
    pub args_offset: usize,
    /// Exact generated field or role token.
    pub token: *const TypeBridgeProjectedTokenV1,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 complete nominal create-argument graph descriptor.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeGeneratedCreateArgsGraphV1 {
    /// Exact size of this graph descriptor.
    pub struct_size: u32,
    /// Frozen generated create-graph layout version.
    pub version: u32,
    /// Complete immutable nominal create-args object.
    pub args: *const c_void,
    /// Exact args object size in bytes.
    pub args_size: usize,
    /// Static table describing every create facet member.
    pub members: *const TypeBridgeGeneratedCreateMemberV1,
    /// Number of member descriptors in the table.
    pub member_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 3],
}

/// Version-1 generated-wrapper input range descriptor.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeGeneratedOpaqueInputV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Generated preflight descriptor version.
    pub version: u32,
    /// One closed `TYPE_BRIDGE_GENERATED_INPUT_*` kind.
    pub kind: u32,
    /// Explicit padding; must be zero.
    pub reserved0: u32,
    /// Input bytes, one opaque object, or pointer-array storage.
    pub pointer: *const c_void,
    /// Byte count for bytes, one for objects, or element count for arrays.
    pub count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 generated-wrapper caller output byte range.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeGeneratedOutputRangeV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Generated preflight descriptor version.
    pub version: u32,
    /// Caller-writable output bytes; this helper never writes them.
    pub pointer: *mut c_void,
    /// Exact output byte count.
    pub length: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

#[derive(Clone, Copy)]
struct MemoryRange {
    start: usize,
    length: usize,
}

impl MemoryRange {
    fn checked(pointer: *const c_void, length: usize) -> Option<Self> {
        if pointer.is_null() || length == 0 {
            return None;
        }
        pointer.addr().checked_add(length)?;
        Some(Self {
            start: pointer.addr(),
            length,
        })
    }

    fn overlaps(self, other: Self) -> bool {
        let left_end = self
            .start
            .checked_add(self.length)
            .expect("preflight ranges were checked at construction");
        let right_end = other
            .start
            .checked_add(other.length)
            .expect("preflight ranges were checked at construction");
        self.start < right_end && other.start < left_end
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DirectOutputPreflight {
    ranges: [MemoryRange; DIRECT_OUTPUT_COUNT_MAX],
    count: usize,
}

impl DirectOutputPreflight {
    pub(crate) fn new(outputs: &[(*mut c_void, usize)]) -> Result<Self, TypeBridgeStatus> {
        if outputs.len() > DIRECT_OUTPUT_COUNT_MAX {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        let mut checked = Self {
            ranges: [MemoryRange {
                start: 0,
                length: 0,
            }; DIRECT_OUTPUT_COUNT_MAX],
            count: 0,
        };
        for &(pointer, length) in outputs {
            if length == 0 {
                return Err(TypeBridgeStatus::InvalidArgument);
            }
            if pointer.is_null() {
                continue;
            }
            let range = MemoryRange::checked(pointer.cast_const(), length)
                .ok_or(TypeBridgeStatus::InvalidArgument)?;
            if checked.overlaps(range) {
                return Err(TypeBridgeStatus::InvalidArgument);
            }
            checked.ranges[checked.count] = range;
            checked.count += 1;
        }
        Ok(checked)
    }

    fn overlaps(&self, input: MemoryRange) -> bool {
        self.ranges[..self.count]
            .iter()
            .any(|output| input.overlaps(*output))
    }

    pub(crate) fn check_bytes(
        &self,
        pointer: *const c_void,
        length: usize,
    ) -> Result<(), TypeBridgeStatus> {
        if length == 0 {
            return Ok(());
        }
        let range =
            MemoryRange::checked(pointer, length).ok_or(TypeBridgeStatus::InvalidArgument)?;
        if self.overlaps(range) {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        Ok(())
    }

    pub(crate) fn check_object_kind(
        &self,
        kind: u32,
        pointer: *const c_void,
    ) -> Result<(), TypeBridgeStatus> {
        if pointer.is_null() {
            return Ok(());
        }
        let length = object_size(kind).ok_or(TypeBridgeStatus::InvalidArgument)?;
        self.check_bytes(pointer, length)
    }

    pub(crate) unsafe fn check_deep_object_kind(
        &self,
        kind: u32,
        pointer: *const c_void,
    ) -> Result<(), TypeBridgeStatus> {
        self.check_object_kind(kind, pointer)?;
        // SAFETY: the caller promises a live object of the stated kind; the
        // complete outer object was checked before deep borrowed traversal.
        unsafe { check_deep_object_ranges(kind, pointer, self) }
    }

    pub(crate) unsafe fn check_pointer_array<T>(
        &self,
        pointer: *const *const T,
        count: usize,
    ) -> Result<(), TypeBridgeStatus> {
        if count == 0 {
            return Ok(());
        }
        let storage_bytes = count
            .checked_mul(size_of::<*const T>())
            .ok_or(TypeBridgeStatus::InvalidArgument)?;
        self.check_bytes(pointer.cast(), storage_bytes)?;
        for index in 0..count {
            // SAFETY: the caller promises the complete pointer array is
            // readable for this call; unaligned reads impose no extra ABI rule.
            let value = unsafe { pointer.add(index).read_unaligned() };
            if value.is_null() {
                continue;
            }
            self.check_bytes(value.cast(), size_of::<T>())?;
        }
        Ok(())
    }

    pub(crate) fn check_package_borrowed_ranges(
        &self,
        package: &SchemaPackageState,
    ) -> Result<(), TypeBridgeStatus> {
        for bytes in [
            package.authority_json.as_slice(),
            package.projection_json.as_slice(),
            package.semantic_fingerprint_json.as_slice(),
            package.binding_fingerprint_json.as_slice(),
            package.managed_scope.as_slice(),
            package.semantic_profile.as_slice(),
        ] {
            self.check_bytes(bytes.as_ptr().cast(), bytes.len())?;
        }
        Ok(())
    }
}

pub(crate) fn direct_output_preflight(
    outputs: &[(*mut c_void, usize)],
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    DirectOutputPreflight::new(outputs)
}

fn object_size(kind: u32) -> Option<usize> {
    match kind {
        GENERATED_INPUT_PROJECTED_TOKEN => Some(size_of::<TypeBridgeProjectedTokenV1>()),
        GENERATED_INPUT_SCHEMA_PACKAGE => Some(size_of::<TypeBridgeSchemaPackage>()),
        GENERATED_INPUT_RUNTIME => Some(size_of::<TypeBridgeRuntime>()),
        GENERATED_INPUT_DATABASE => Some(size_of::<TypeBridgeDatabase>()),
        GENERATED_INPUT_READ_TRANSACTION => Some(size_of::<TypeBridgeReadTransaction>()),
        GENERATED_INPUT_WRITE_TRANSACTION => Some(size_of::<TypeBridgeWriteTransaction>()),
        GENERATED_INPUT_CANCELLATION => Some(size_of::<TypeBridgeCancellation>()),
        GENERATED_INPUT_PROJECTED_VALUE => Some(size_of::<TypeBridgeProjectedValue>()),
        GENERATED_INPUT_PROJECTED_REFERENCE => Some(size_of::<TypeBridgeProjectedReference>()),
        GENERATED_INPUT_PROJECTED_CREATE => Some(size_of::<TypeBridgeProjectedCreate>()),
        GENERATED_INPUT_PROJECTED_THING => Some(size_of::<TypeBridgeProjectedThing>()),
        GENERATED_INPUT_DIAGNOSTICS => Some(size_of::<TypeBridgeDiagnostics>()),
        GENERATED_INPUT_EXECUTION_DIAGNOSTICS => Some(size_of::<TypeBridgeExecutionDiagnostics>()),
        GENERATED_INPUT_QUERY_SESSION => Some(size_of::<TypeBridgeQuerySession>()),
        GENERATED_INPUT_QUERY_BINDING => Some(size_of::<TypeBridgeQueryBinding>()),
        GENERATED_INPUT_QUERY_FIELD => Some(size_of::<TypeBridgeQueryField>()),
        GENERATED_INPUT_QUERY_ROLE => Some(size_of::<TypeBridgeQueryRole>()),
        GENERATED_INPUT_QUERY_PREDICATE => Some(size_of::<TypeBridgeQueryPredicate>()),
        GENERATED_INPUT_QUERY_ORDER => Some(size_of::<TypeBridgeQueryOrder>()),
        GENERATED_INPUT_QUERY_SELECTION => Some(size_of::<TypeBridgeQuerySelection>()),
        GENERATED_INPUT_QUERY => Some(size_of::<TypeBridgeQuery>()),
        GENERATED_INPUT_QUERY_TERMINAL => Some(size_of::<TypeBridgeQueryTerminal>()),
        GENERATED_INPUT_QUERY_RESULT => Some(size_of::<TypeBridgeQueryResult>()),
        GENERATED_INPUT_QUERY_REMOTE_CONTEXT => Some(size_of::<TypeBridgeQueryRemoteContext>()),
        GENERATED_INPUT_QUERY_REMOTE_PENDING => Some(size_of::<TypeBridgeQueryRemotePending>()),
        GENERATED_INPUT_QUERY_REMOTE_CLAIM => Some(size_of::<TypeBridgeQueryRemoteClaim>()),
        GENERATED_INPUT_QUERY_FUNCTION => Some(size_of::<TypeBridgeQueryFunction>()),
        GENERATED_INPUT_QUERY_FUNCTION_VALUE => Some(size_of::<TypeBridgeQueryFunctionValue>()),
        GENERATED_INPUT_QUERY_FUNCTION_CALL => Some(size_of::<TypeBridgeQueryFunctionCall>()),
        GENERATED_INPUT_PROJECTED_BATCH_BUILDER => {
            Some(size_of::<TypeBridgeProjectedBatchBuilder>())
        }
        GENERATED_INPUT_PROJECTED_BATCH => Some(size_of::<TypeBridgeProjectedBatch>()),
        GENERATED_INPUT_PROJECTED_BATCH_RESULT => Some(size_of::<TypeBridgeProjectedBatchResult>()),
        GENERATED_INPUT_CANONICAL_BYTES => Some(size_of::<TypeBridgeCanonicalBytes>()),
        GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER => {
            Some(size_of::<TypeBridgeCanonicalArchiveBuilder>())
        }
        GENERATED_INPUT_CANONICAL_ARCHIVE => Some(size_of::<TypeBridgeCanonicalArchive>()),
        GENERATED_INPUT_PROJECTED_STRUCT => Some(size_of::<TypeBridgeProjectedStruct>()),
        GENERATED_INPUT_PROJECTED_STRUCT_MEMBER => {
            Some(size_of::<TypeBridgeProjectedStructMember>())
        }
        GENERATED_INPUT_PROJECTED_CODEC_OPTIONS => {
            Some(size_of::<TypeBridgeProjectedCodecOptionsV1>())
        }
        _ => None,
    }
}

fn validate_layout(
    struct_size: u32,
    expected_size: usize,
    version: u32,
    reserved: [u64; 4],
) -> bool {
    struct_size as usize == expected_size
        && version == GENERATED_PREFLIGHT_VERSION
        && reserved == [0; 4]
}

unsafe fn copied_outputs(
    outputs: *const TypeBridgeGeneratedOutputRangeV1,
    output_count: usize,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    if outputs.is_null() || !(1..=GENERATED_PREFLIGHT_OUTPUT_COUNT_MAX).contains(&output_count) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    let mut output_pairs = [(std::ptr::null_mut(), 0_usize); GENERATED_PREFLIGHT_OUTPUT_COUNT_MAX];
    for (index, slot) in output_pairs.iter_mut().enumerate().take(output_count) {
        // SAFETY: the caller promises a readable descriptor slice; unaligned
        // reads avoid imposing a Rust alignment requirement on C storage.
        let output = unsafe { outputs.add(index).read_unaligned() };
        if !validate_layout(
            output.struct_size,
            size_of::<TypeBridgeGeneratedOutputRangeV1>(),
            output.version,
            output.reserved,
        ) {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        if output.pointer.is_null() {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        *slot = (output.pointer, output.length);
    }
    DirectOutputPreflight::new(&output_pairs[..output_count])
}

unsafe fn pointer_array_aliases<T>(
    pointer: *const c_void,
    count: usize,
    outputs: &DirectOutputPreflight,
) -> Result<bool, TypeBridgeStatus> {
    if count == 0 {
        return Ok(false);
    }
    let storage_bytes = count
        .checked_mul(size_of::<*const T>())
        .ok_or(TypeBridgeStatus::InvalidArgument)?;
    let storage =
        MemoryRange::checked(pointer, storage_bytes).ok_or(TypeBridgeStatus::InvalidArgument)?;
    if outputs.overlaps(storage) {
        return Ok(true);
    }
    let pointers = pointer.cast::<*const T>();
    for index in 0..count {
        // SAFETY: the array storage was checked and is caller-readable.
        let value = unsafe { pointers.add(index).read_unaligned() };
        if value.is_null() {
            continue;
        }
        let range = MemoryRange::checked(value.cast(), size_of::<T>())
            .ok_or(TypeBridgeStatus::InvalidArgument)?;
        if outputs.overlaps(range) {
            return Ok(true);
        }
    }
    Ok(false)
}

unsafe fn check_deep_object_ranges(
    kind: u32,
    pointer: *const c_void,
    outputs: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        return Ok(());
    }
    match kind {
        GENERATED_INPUT_SCHEMA_PACKAGE => {
            // SAFETY: the complete package handle was checked before this dispatch.
            let package = unsafe { &*pointer.cast::<TypeBridgeSchemaPackage>() };
            outputs.check_package_borrowed_ranges(package.state())
        }
        GENERATED_INPUT_PROJECTED_VALUE => {
            // SAFETY: the complete projected-value handle was checked before this dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedValue>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_REFERENCE => {
            // SAFETY: the complete projected-reference handle was checked before this dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedReference>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_CREATE => {
            // SAFETY: the complete projected-create handle was checked before this dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedCreate>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_THING => {
            // SAFETY: the complete projected-thing handle was checked before this dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedThing>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_EXECUTION_DIAGNOSTICS => {
            // SAFETY: the complete execution-diagnostics handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeExecutionDiagnostics>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_CANONICAL_BYTES => {
            // SAFETY: the complete canonical-byte handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeCanonicalBytes>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER => {
            // SAFETY: the complete archive builder was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeCanonicalArchiveBuilder>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_CANONICAL_ARCHIVE => {
            // SAFETY: the complete archive handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeCanonicalArchive>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_STRUCT => {
            // SAFETY: the complete projected struct was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedStruct>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_STRUCT_MEMBER => {
            // SAFETY: the complete struct-member handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedStructMember>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_CODEC_OPTIONS => {
            // SAFETY: the complete options object was checked before dispatch.
            let options = unsafe {
                pointer
                    .cast::<TypeBridgeProjectedCodecOptionsV1>()
                    .read_unaligned()
            };
            // SAFETY: the caller promises the optional cancellation owner remains live.
            unsafe {
                outputs.check_deep_object_kind(
                    GENERATED_INPUT_CANCELLATION,
                    options.cancellation.cast(),
                )
            }
        }
        GENERATED_INPUT_QUERY_SESSION => {
            // SAFETY: the complete query-session handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQuerySession>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_BINDING => {
            // SAFETY: the complete query-binding handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryBinding>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_FIELD => {
            // SAFETY: the complete query-field handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryField>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_ROLE => {
            // SAFETY: the complete query-role handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryRole>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_PREDICATE => {
            // SAFETY: the complete query-predicate handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryPredicate>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_ORDER => {
            // SAFETY: the complete query-order handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryOrder>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_SELECTION => {
            // SAFETY: the complete query-selection handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQuerySelection>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY => {
            // SAFETY: the complete query handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQuery>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_TERMINAL => {
            // SAFETY: the complete query-terminal handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryTerminal>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_RESULT => {
            // SAFETY: the complete query-result handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryResult>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_REMOTE_CONTEXT => {
            // SAFETY: the complete remote-context handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryRemoteContext>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_REMOTE_PENDING => {
            // SAFETY: the complete pending-request handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryRemotePending>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_REMOTE_CLAIM => {
            // SAFETY: the complete reply-claim handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryRemoteClaim>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_FUNCTION => {
            // SAFETY: the complete function handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryFunction>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_FUNCTION_VALUE => {
            // SAFETY: the complete function-value handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryFunctionValue>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_QUERY_FUNCTION_CALL => {
            // SAFETY: the complete function-call handle was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeQueryFunctionCall>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_BATCH_BUILDER => {
            // SAFETY: the complete projected-batch builder was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedBatchBuilder>() }
                .check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_BATCH => {
            // SAFETY: the complete immutable projected batch was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedBatch>() }.check_borrowed_ranges(outputs)
        }
        GENERATED_INPUT_PROJECTED_BATCH_RESULT => {
            // SAFETY: the complete projected-batch result was checked before dispatch.
            unsafe { &*pointer.cast::<TypeBridgeProjectedBatchResult>() }
                .check_borrowed_ranges(outputs)
        }
        _ => Ok(()),
    }
}

unsafe fn projected_resource_measure(
    kind: u32,
    pointer: *const c_void,
) -> Option<ProjectedResourceMeasure> {
    if pointer.is_null() {
        return None;
    }
    match kind {
        GENERATED_INPUT_PROJECTED_VALUE => {
            // SAFETY: the caller retains one complete immutable handle object.
            Some(
                unsafe { &*pointer.cast::<TypeBridgeProjectedValue>() }
                    .value()
                    .resource_measure(),
            )
        }
        GENERATED_INPUT_PROJECTED_REFERENCE => {
            // SAFETY: the caller retains one complete immutable handle object.
            Some(
                unsafe { &*pointer.cast::<TypeBridgeProjectedReference>() }
                    .value
                    .resource_measure(),
            )
        }
        GENERATED_INPUT_PROJECTED_CREATE => {
            // SAFETY: the caller retains one complete immutable handle object.
            Some(
                unsafe { &*pointer.cast::<TypeBridgeProjectedCreate>() }
                    .value
                    .resource_measure(),
            )
        }
        GENERATED_INPUT_PROJECTED_THING => {
            // SAFETY: the caller retains one complete immutable handle object.
            Some(
                unsafe { &*pointer.cast::<TypeBridgeProjectedThing>() }
                    .value
                    .resource_measure(),
            )
        }
        _ => None,
    }
}

fn add_projected_measure(
    members: &mut usize,
    bytes: &mut usize,
    measure: ProjectedResourceMeasure,
) -> Result<(), TypeBridgeStatus> {
    *members = members
        .checked_add(measure.members())
        .filter(|value| *value <= type_bridge_orm::MAX_PROJECTED_MODEL_MEMBERS)
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    *bytes = bytes
        .checked_add(measure.bytes())
        .filter(|value| *value <= type_bridge_orm::MAX_PROJECTED_MODEL_BYTES)
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    Ok(())
}

fn generated_create_member_handle_kind(kind: u32) -> Option<(u32, bool)> {
    match kind {
        GENERATED_CREATE_MEMBER_VALUE_SCALAR => Some((GENERATED_INPUT_PROJECTED_VALUE, false)),
        GENERATED_CREATE_MEMBER_VALUE_SEQUENCE => Some((GENERATED_INPUT_PROJECTED_VALUE, true)),
        GENERATED_CREATE_MEMBER_REFERENCE_SCALAR => {
            Some((GENERATED_INPUT_PROJECTED_REFERENCE, false))
        }
        GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE => {
            Some((GENERATED_INPUT_PROJECTED_REFERENCE, true))
        }
        _ => None,
    }
}

enum GeneratedCreateGraphVisit {
    Structure(usize),
    Handle(u32, *const c_void),
}

fn check_generated_create_graph_range(
    outputs: &DirectOutputPreflight,
    pointer: *const c_void,
    length: usize,
    fence_outputs: bool,
) -> Result<(), TypeBridgeStatus> {
    MemoryRange::checked(pointer, length).ok_or(TypeBridgeStatus::InvalidArgument)?;
    if fence_outputs {
        outputs.check_bytes(pointer, length)?;
    }
    Ok(())
}

unsafe fn walk_generated_create_args_graph(
    pointer: *const c_void,
    count: usize,
    outputs: &DirectOutputPreflight,
    raw_handle_limit: usize,
    fence_outputs: bool,
    mut visit: impl FnMut(GeneratedCreateGraphVisit) -> Result<(), TypeBridgeStatus>,
) -> Result<(usize, usize), TypeBridgeStatus> {
    if count != 1 || pointer.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    check_generated_create_graph_range(
        outputs,
        pointer,
        size_of::<TypeBridgeGeneratedCreateArgsGraphV1>(),
        fence_outputs,
    )?;
    // SAFETY: the complete graph descriptor range was checked above.
    let graph = unsafe {
        pointer
            .cast::<TypeBridgeGeneratedCreateArgsGraphV1>()
            .read_unaligned()
    };
    if graph.struct_size as usize != size_of::<TypeBridgeGeneratedCreateArgsGraphV1>()
        || graph.version != GENERATED_CREATE_GRAPH_VERSION
        || graph.reserved != [0; 3]
        || graph.args.is_null()
        || graph.args_size == 0
        || graph.args_size > GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX
        || graph.member_count > GENERATED_CREATE_MEMBER_COUNT_MAX
        || (graph.member_count != 0 && graph.members.is_null())
    {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    check_generated_create_graph_range(outputs, graph.args, graph.args_size, fence_outputs)?;
    if graph.member_count != 0 {
        let table_bytes = graph
            .member_count
            .checked_mul(size_of::<TypeBridgeGeneratedCreateMemberV1>())
            .filter(|bytes| *bytes <= GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX)
            .ok_or(TypeBridgeStatus::ResourceLimit)?;
        check_generated_create_graph_range(
            outputs,
            graph.members.cast(),
            table_bytes,
            fence_outputs,
        )?;
    }
    visit(GeneratedCreateGraphVisit::Structure(graph.member_count))?;

    let mut raw_handle_count = 0_usize;
    for member_index in 0..graph.member_count {
        // SAFETY: the complete bounded member table was checked above.
        let member = unsafe { graph.members.add(member_index).read_unaligned() };
        let Some((handle_kind, sequence)) = generated_create_member_handle_kind(member.kind) else {
            return Err(TypeBridgeStatus::InvalidArgument);
        };
        if member.struct_size as usize != size_of::<TypeBridgeGeneratedCreateMemberV1>()
            || member.version != GENERATED_CREATE_GRAPH_VERSION
            || member.reserved0 != 0
            || member.reserved != [0; 4]
            || member.token.is_null()
        {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        check_generated_create_graph_range(
            outputs,
            member.token.cast(),
            size_of::<TypeBridgeProjectedTokenV1>(),
            fence_outputs,
        )?;
        let payload_end = member
            .args_offset
            .checked_add(size_of::<*const c_void>())
            .ok_or(TypeBridgeStatus::InvalidArgument)?;
        if payload_end > graph.args_size {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        // SAFETY: the complete args object and this in-bounds pointer slot were checked above.
        let payload = unsafe {
            graph
                .args
                .cast::<u8>()
                .add(member.args_offset)
                .cast::<*const c_void>()
                .read_unaligned()
        };
        if !sequence {
            if !payload.is_null() {
                raw_handle_count = raw_handle_count
                    .checked_add(1)
                    .filter(|value| *value <= raw_handle_limit)
                    .ok_or(TypeBridgeStatus::ResourceLimit)?;
                let object_bytes = object_size(handle_kind)
                    .expect("closed generated create handle kinds have object sizes");
                check_generated_create_graph_range(outputs, payload, object_bytes, fence_outputs)?;
                visit(GeneratedCreateGraphVisit::Handle(handle_kind, payload))?;
            }
            continue;
        }

        let mut chunk = payload.cast::<TypeBridgeGeneratedCreateHandleChunkV1>();
        let mut cycle_anchor = chunk;
        let mut cycle_power = 1_usize;
        let mut cycle_length = 0_usize;
        while !chunk.is_null() {
            check_generated_create_graph_range(
                outputs,
                chunk.cast(),
                size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
                fence_outputs,
            )?;
            // SAFETY: the complete chunk object range was checked above.
            let snapshot = unsafe { chunk.read_unaligned() };
            if snapshot.struct_size != size_of::<TypeBridgeGeneratedCreateHandleChunkV1>()
                || snapshot.version != GENERATED_CREATE_GRAPH_VERSION
                || snapshot.reserved != [0; 4]
                || snapshot.count == 0
                || snapshot.values.is_null()
            {
                return Err(TypeBridgeStatus::InvalidArgument);
            }
            let array_bytes = snapshot
                .count
                .checked_mul(size_of::<*const c_void>())
                .filter(|bytes| *bytes <= GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
            raw_handle_count = raw_handle_count
                .checked_add(snapshot.count)
                .filter(|value| *value <= raw_handle_limit)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
            check_generated_create_graph_range(
                outputs,
                snapshot.values.cast(),
                array_bytes,
                fence_outputs,
            )?;
            for handle_index in 0..snapshot.count {
                // SAFETY: the complete bounded pointer array was checked above.
                let handle = unsafe { snapshot.values.add(handle_index).read_unaligned() };
                if handle.is_null() {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
                let object_bytes = object_size(handle_kind)
                    .expect("closed generated create handle kinds have object sizes");
                check_generated_create_graph_range(outputs, handle, object_bytes, fence_outputs)?;
                visit(GeneratedCreateGraphVisit::Handle(handle_kind, handle))?;
            }
            let next = snapshot.next;
            if !next.is_null() {
                cycle_length = cycle_length
                    .checked_add(1)
                    .ok_or(TypeBridgeStatus::ResourceLimit)?;
                if next == cycle_anchor {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
                if cycle_length == cycle_power {
                    cycle_anchor = next;
                    cycle_power = cycle_power
                        .checked_mul(2)
                        .ok_or(TypeBridgeStatus::ResourceLimit)?;
                    cycle_length = 0;
                }
            }
            chunk = next;
        }
    }
    Ok((graph.member_count, raw_handle_count))
}

unsafe fn check_pointer_array_deep_ranges<T>(
    pointer: *const c_void,
    count: usize,
    element_kind: u32,
    outputs: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    let pointers = pointer.cast::<*const T>();
    for index in 0..count {
        // SAFETY: the complete bounded pointer-array storage was checked first.
        let value = unsafe { pointers.add(index).read_unaligned() };
        if !value.is_null() {
            // SAFETY: the complete pointee object was checked in the outer pass.
            unsafe { check_deep_object_ranges(element_kind, value.cast(), outputs) }?;
        }
    }
    Ok(())
}

unsafe fn preflight(
    inputs: *const TypeBridgeGeneratedOpaqueInputV1,
    input_count: usize,
    outputs: *const TypeBridgeGeneratedOutputRangeV1,
    output_count: usize,
) -> Result<(), TypeBridgeStatus> {
    if input_count > MAX_CANONICAL_COLLECTION_LEN || (input_count != 0 && inputs.is_null()) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: output descriptor storage is caller-readable for this call.
    let output_ranges = unsafe { copied_outputs(outputs, output_count) }?;

    let output_descriptor_bytes = output_count
        .checked_mul(size_of::<TypeBridgeGeneratedOutputRangeV1>())
        .ok_or(TypeBridgeStatus::InvalidArgument)?;
    let output_descriptors = MemoryRange::checked(outputs.cast(), output_descriptor_bytes)
        .ok_or(TypeBridgeStatus::InvalidArgument)?;
    if output_ranges.overlaps(output_descriptors) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if input_count != 0 {
        let input_descriptor_bytes = input_count
            .checked_mul(size_of::<TypeBridgeGeneratedOpaqueInputV1>())
            .ok_or(TypeBridgeStatus::InvalidArgument)?;
        let input_descriptors = MemoryRange::checked(inputs.cast(), input_descriptor_bytes)
            .ok_or(TypeBridgeStatus::InvalidArgument)?;
        if output_ranges.overlaps(input_descriptors) {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
    }

    let mut pointee_inspection_count = 0_usize;
    for index in 0..input_count {
        // SAFETY: the caller promises a readable descriptor slice. This first
        // pass establishes the aggregate budget before any pointer-array walk.
        let input = unsafe { inputs.add(index).read_unaligned() };
        if !validate_layout(
            input.struct_size,
            size_of::<TypeBridgeGeneratedOpaqueInputV1>(),
            input.version,
            input.reserved,
        ) || input.reserved0 != 0
        {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        if matches!(
            input.kind,
            GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY
                | GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY
        ) {
            pointee_inspection_count = pointee_inspection_count
                .checked_add(input.count)
                .filter(|count| *count <= MAX_CANONICAL_COLLECTION_LEN)
                .ok_or(TypeBridgeStatus::ResourceLimit)?;
        }
    }

    // Fence every directly described range, pointer-array storage, and opaque
    // handle object before reading cached measures from any opaque handle.
    // This preserves InvalidArgument precedence for an output forged into an
    // outer object while still deferring nested heap-range walks until after
    // the aggregate semantic resource budget is known.
    for index in 0..input_count {
        // SAFETY: the bounded descriptor slice remains caller-readable.
        let input = unsafe { inputs.add(index).read_unaligned() };
        match input.kind {
            GENERATED_INPUT_BYTES => {
                if input.count != 0 {
                    output_ranges.check_bytes(input.pointer, input.count)?;
                }
            }
            GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY => {
                // SAFETY: this kind explicitly describes an immutable pointer array.
                if unsafe {
                    pointer_array_aliases::<TypeBridgeProjectedValue>(
                        input.pointer,
                        input.count,
                        &output_ranges,
                    )
                }? {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
            }
            GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY => {
                // SAFETY: this kind explicitly describes an immutable pointer array.
                if unsafe {
                    pointer_array_aliases::<TypeBridgeProjectedReference>(
                        input.pointer,
                        input.count,
                        &output_ranges,
                    )
                }? {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
            }
            GENERATED_INPUT_CREATE_ARGS_GRAPH => {
                let remaining_raw_handles = MAX_CANONICAL_COLLECTION_LEN
                    .checked_sub(pointee_inspection_count)
                    .ok_or(TypeBridgeStatus::ResourceLimit)?;
                // SAFETY: generated graph storage is immutable for this read-only call.
                let (_, raw_handles) = unsafe {
                    walk_generated_create_args_graph(
                        input.pointer,
                        input.count,
                        &output_ranges,
                        remaining_raw_handles,
                        true,
                        |_| Ok(()),
                    )
                }?;
                pointee_inspection_count = pointee_inspection_count
                    .checked_add(raw_handles)
                    .filter(|value| *value <= MAX_CANONICAL_COLLECTION_LEN)
                    .ok_or(TypeBridgeStatus::ResourceLimit)?;
            }
            kind => {
                if input.count != 1 {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
                let object_bytes = object_size(kind).ok_or(TypeBridgeStatus::InvalidArgument)?;
                output_ranges.check_bytes(input.pointer, object_bytes)?;
            }
        }
    }

    let mut projected_graph_members = 0_usize;
    let mut projected_graph_bytes = 0_usize;
    for index in 0..input_count {
        // SAFETY: the caller promises the bounded descriptor slice is readable.
        let input = unsafe { inputs.add(index).read_unaligned() };
        let element_kind = match input.kind {
            GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY => Some(GENERATED_INPUT_PROJECTED_VALUE),
            GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY => {
                Some(GENERATED_INPUT_PROJECTED_REFERENCE)
            }
            _ => None,
        };
        if input.kind == GENERATED_INPUT_CREATE_ARGS_GRAPH {
            // SAFETY: the generated graph and every described immutable object
            // remain readable for this read-only aggregate measurement pass.
            unsafe {
                walk_generated_create_args_graph(
                    input.pointer,
                    input.count,
                    &output_ranges,
                    MAX_CANONICAL_COLLECTION_LEN,
                    false,
                    |visit| {
                        match visit {
                            GeneratedCreateGraphVisit::Structure(member_count) => {
                                projected_graph_members = projected_graph_members
                                    .checked_add(1)
                                    .and_then(|value| value.checked_add(member_count))
                                    .filter(|value| {
                                        *value <= type_bridge_orm::MAX_PROJECTED_MODEL_MEMBERS
                                    })
                                    .ok_or(TypeBridgeStatus::ResourceLimit)?;
                            }
                            GeneratedCreateGraphVisit::Handle(kind, pointer) => {
                                // SAFETY: the walker checked the complete opaque handle object.
                                if let Some(measure) = projected_resource_measure(kind, pointer) {
                                    add_projected_measure(
                                        &mut projected_graph_members,
                                        &mut projected_graph_bytes,
                                        measure,
                                    )?;
                                }
                            }
                        }
                        Ok(())
                    },
                )
            }?;
        } else if let Some(element_kind) = element_kind {
            if input.pointer.is_null() {
                continue;
            }
            let pointers = input.pointer.cast::<*const c_void>();
            for pointer_index in 0..input.count {
                // SAFETY: the first pass bounded the array and caller promises its readability.
                let pointer = unsafe { pointers.add(pointer_index).read_unaligned() };
                // SAFETY: every non-null pointee is one retained immutable opaque handle.
                if let Some(measure) = unsafe { projected_resource_measure(element_kind, pointer) }
                {
                    add_projected_measure(
                        &mut projected_graph_members,
                        &mut projected_graph_bytes,
                        measure,
                    )?;
                }
            }
        } else {
            // SAFETY: singleton projected kinds describe one retained opaque handle.
            if let Some(measure) = unsafe { projected_resource_measure(input.kind, input.pointer) }
            {
                add_projected_measure(
                    &mut projected_graph_members,
                    &mut projected_graph_bytes,
                    measure,
                )?;
            }
        }
    }

    for index in 0..input_count {
        // SAFETY: the caller promises a readable descriptor slice.
        let input = unsafe { inputs.add(index).read_unaligned() };
        if !validate_layout(
            input.struct_size,
            size_of::<TypeBridgeGeneratedOpaqueInputV1>(),
            input.version,
            input.reserved,
        ) || input.reserved0 != 0
        {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        if input.kind == GENERATED_INPUT_BYTES {
            continue;
        }
        if input.kind == GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY {
            // SAFETY: outer pointer-array storage and every complete pointee were checked above.
            unsafe {
                check_pointer_array_deep_ranges::<TypeBridgeProjectedValue>(
                    input.pointer,
                    input.count,
                    GENERATED_INPUT_PROJECTED_VALUE,
                    &output_ranges,
                )
            }?;
            continue;
        }
        if input.kind == GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY {
            // SAFETY: outer pointer-array storage and every complete pointee were checked above.
            unsafe {
                check_pointer_array_deep_ranges::<TypeBridgeProjectedReference>(
                    input.pointer,
                    input.count,
                    GENERATED_INPUT_PROJECTED_REFERENCE,
                    &output_ranges,
                )
            }?;
            continue;
        }
        if input.kind == GENERATED_INPUT_CREATE_ARGS_GRAPH {
            // SAFETY: the aggregate pass validated and bounded this immutable
            // graph; the second pass fences every deeply borrowed handle range.
            unsafe {
                walk_generated_create_args_graph(
                    input.pointer,
                    input.count,
                    &output_ranges,
                    MAX_CANONICAL_COLLECTION_LEN,
                    true,
                    |visit| match visit {
                        GeneratedCreateGraphVisit::Structure(_) => Ok(()),
                        GeneratedCreateGraphVisit::Handle(kind, pointer) => {
                            // SAFETY: the walker checked the complete opaque handle object first.
                            check_deep_object_ranges(kind, pointer, &output_ranges)
                        }
                    },
                )
            }?;
            continue;
        }
        // SAFETY: this singleton's complete outer object range was checked above.
        unsafe { check_deep_object_ranges(input.kind, input.pointer, &output_ranges) }?;
    }
    Ok(())
}

/// Validate all true Rust-owned input ranges against one or two caller output ranges.
///
/// This generated-only helper is read-only: every return path leaves all
/// described bytes untouched. Generated wrappers call it before initializing
/// any caller output slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_generated_opaque_alias_preflight_v1(
    inputs: *const TypeBridgeGeneratedOpaqueInputV1,
    input_count: usize,
    outputs: *const TypeBridgeGeneratedOutputRangeV1,
    output_count: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        // SAFETY: the generated caller retains both descriptor slices and every
        // described immutable input for the duration of this read-only call.
        match unsafe { preflight(inputs, input_count, outputs, output_count) } {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::ptr::{self, NonNull};

    use super::*;

    fn input(kind: u32, pointer: *const c_void, count: usize) -> TypeBridgeGeneratedOpaqueInputV1 {
        TypeBridgeGeneratedOpaqueInputV1 {
            struct_size: size_of::<TypeBridgeGeneratedOpaqueInputV1>() as u32,
            version: GENERATED_PREFLIGHT_VERSION,
            kind,
            reserved0: 0,
            pointer,
            count,
            reserved: [0; 4],
        }
    }

    fn output(pointer: *mut c_void, length: usize) -> TypeBridgeGeneratedOutputRangeV1 {
        TypeBridgeGeneratedOutputRangeV1 {
            struct_size: size_of::<TypeBridgeGeneratedOutputRangeV1>() as u32,
            version: GENERATED_PREFLIGHT_VERSION,
            pointer,
            length,
            reserved: [0; 4],
        }
    }

    #[test]
    fn legacy_object_sizes_remain_exact_and_batch_kinds_extend_the_closed_table() {
        let legacy_sizes = [
            None,
            Some(size_of::<TypeBridgeProjectedTokenV1>()),
            Some(size_of::<TypeBridgeSchemaPackage>()),
            Some(size_of::<TypeBridgeRuntime>()),
            Some(size_of::<TypeBridgeDatabase>()),
            Some(size_of::<TypeBridgeReadTransaction>()),
            Some(size_of::<TypeBridgeWriteTransaction>()),
            Some(size_of::<TypeBridgeCancellation>()),
            Some(size_of::<TypeBridgeProjectedValue>()),
            Some(size_of::<TypeBridgeProjectedReference>()),
            Some(size_of::<TypeBridgeProjectedCreate>()),
            Some(size_of::<TypeBridgeProjectedThing>()),
            None,
            None,
            Some(size_of::<TypeBridgeDiagnostics>()),
            Some(size_of::<TypeBridgeExecutionDiagnostics>()),
            None,
            Some(size_of::<TypeBridgeQuerySession>()),
            Some(size_of::<TypeBridgeQueryBinding>()),
            Some(size_of::<TypeBridgeQueryField>()),
            Some(size_of::<TypeBridgeQueryRole>()),
            Some(size_of::<TypeBridgeQueryPredicate>()),
            Some(size_of::<TypeBridgeQueryOrder>()),
            Some(size_of::<TypeBridgeQuerySelection>()),
            Some(size_of::<TypeBridgeQuery>()),
            Some(size_of::<TypeBridgeQueryTerminal>()),
            Some(size_of::<TypeBridgeQueryResult>()),
            Some(size_of::<TypeBridgeQueryRemoteContext>()),
            Some(size_of::<TypeBridgeQueryRemotePending>()),
            Some(size_of::<TypeBridgeQueryRemoteClaim>()),
            Some(size_of::<TypeBridgeQueryFunction>()),
            Some(size_of::<TypeBridgeQueryFunctionValue>()),
            Some(size_of::<TypeBridgeQueryFunctionCall>()),
        ];
        for (index, expected) in legacy_sizes.into_iter().enumerate() {
            assert_eq!(object_size(index as u32 + 1), expected);
        }

        assert_eq!(GENERATED_INPUT_PROJECTED_BATCH_BUILDER, 34);
        assert_eq!(GENERATED_INPUT_PROJECTED_BATCH, 35);
        assert_eq!(GENERATED_INPUT_PROJECTED_BATCH_RESULT, 36);
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_BATCH_BUILDER),
            Some(size_of::<TypeBridgeProjectedBatchBuilder>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_BATCH),
            Some(size_of::<TypeBridgeProjectedBatch>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_BATCH_RESULT),
            Some(size_of::<TypeBridgeProjectedBatchResult>()),
        );
        assert_eq!(GENERATED_INPUT_CANONICAL_BYTES, 37);
        assert_eq!(GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER, 38);
        assert_eq!(GENERATED_INPUT_CANONICAL_ARCHIVE, 39);
        assert_eq!(GENERATED_INPUT_PROJECTED_STRUCT, 40);
        assert_eq!(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER, 41);
        assert_eq!(GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, 42);
        assert_eq!(
            object_size(GENERATED_INPUT_CANONICAL_BYTES),
            Some(size_of::<TypeBridgeCanonicalBytes>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER),
            Some(size_of::<TypeBridgeCanonicalArchiveBuilder>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_CANONICAL_ARCHIVE),
            Some(size_of::<TypeBridgeCanonicalArchive>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_STRUCT),
            Some(size_of::<TypeBridgeProjectedStruct>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_STRUCT_MEMBER),
            Some(size_of::<TypeBridgeProjectedStructMember>()),
        );
        assert_eq!(
            object_size(GENERATED_INPUT_PROJECTED_CODEC_OPTIONS),
            Some(size_of::<TypeBridgeProjectedCodecOptionsV1>()),
        );
        assert_eq!(object_size(43), None);
        assert_eq!(object_size(u32::MAX), None);
    }

    #[test]
    fn token_and_bytes_use_exact_ranges_and_never_write_outputs() {
        let token = TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: 1,
            kind: 1,
            ordinal: 0,
            projection_digest: [7; 32],
            reserved: [0; 4],
        };
        let mut caller_output = [0xa5_u8; 16];
        let inputs = [
            input(
                GENERATED_INPUT_PROJECTED_TOKEN,
                (&token as *const TypeBridgeProjectedTokenV1).cast(),
                1,
            ),
            input(GENERATED_INPUT_BYTES, b"0x10".as_ptr().cast(), 4),
        ];
        let outputs = [output(
            caller_output.as_mut_ptr().cast(),
            caller_output.len(),
        )];
        assert_eq!(
            // SAFETY: all descriptors and described ranges remain live.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    inputs.as_ptr(),
                    inputs.len(),
                    outputs.as_ptr(),
                    outputs.len(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(caller_output, [0xa5; 16]);

        let interior = unsafe {
            (&token as *const TypeBridgeProjectedTokenV1)
                .cast::<u8>()
                .add(4)
                .cast_mut()
        };
        let outputs = [output(interior.cast(), 1)];
        let before = token;
        assert_eq!(
            // SAFETY: the hostile output range is allocated inside the token.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    inputs.as_ptr(),
                    inputs.len(),
                    outputs.as_ptr(),
                    outputs.len(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(token, before);
    }

    #[test]
    fn aggregate_array_budget_accepts_exact_ceiling_and_rejects_max_plus_one_read_only() {
        let mut caller_output = usize::MAX;
        let outputs = [output(
            (&mut caller_output as *mut usize).cast(),
            size_of::<usize>(),
        )];
        let ceiling = vec![ptr::null::<TypeBridgeProjectedValue>(); MAX_CANONICAL_COLLECTION_LEN];
        let ceiling_input = [input(
            GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
            ceiling.as_ptr().cast(),
            ceiling.len(),
        )];
        assert_eq!(
            // SAFETY: every array element is readable and NULL.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    ceiling_input.as_ptr(),
                    ceiling_input.len(),
                    outputs.as_ptr(),
                    outputs.len(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(caller_output, usize::MAX);

        let mut ceiling_with_last_alias = ceiling;
        ceiling_with_last_alias[MAX_CANONICAL_COLLECTION_LEN - 1] =
            (&caller_output as *const usize).cast();
        let ceiling_alias_input = [input(
            GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
            ceiling_with_last_alias.as_ptr().cast(),
            ceiling_with_last_alias.len(),
        )];
        assert_eq!(
            // SAFETY: every pointer-array element is readable; the final one
            // deliberately describes the caller output and must be inspected.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    ceiling_alias_input.as_ptr(),
                    ceiling_alias_input.len(),
                    outputs.as_ptr(),
                    outputs.len(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(caller_output, usize::MAX);

        let over_ceiling_input = [
            input(
                GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
                NonNull::<*const TypeBridgeProjectedValue>::dangling()
                    .as_ptr()
                    .cast(),
                MAX_CANONICAL_COLLECTION_LEN,
            ),
            input(
                GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY,
                NonNull::<*const TypeBridgeProjectedReference>::dangling()
                    .as_ptr()
                    .cast(),
                1,
            ),
        ];
        assert_eq!(
            // SAFETY: descriptor and array storage are readable. Aggregate
            // rejection occurs before either pointer-array element is read.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    over_ceiling_input.as_ptr(),
                    over_ceiling_input.len(),
                    outputs.as_ptr(),
                    outputs.len(),
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(caller_output, usize::MAX);
    }

    #[test]
    fn empty_inputs_and_disjoint_outputs_are_enforced_without_writes() {
        let mut caller_output = [0x5a_u8; 16];
        let overlapping_outputs = [
            output(caller_output.as_mut_ptr().cast(), 8),
            output(unsafe { caller_output.as_mut_ptr().add(1) }.cast(), 8),
        ];
        assert_eq!(
            // SAFETY: both descriptors and output storage remain live.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    ptr::null(),
                    0,
                    overlapping_outputs.as_ptr(),
                    overlapping_outputs.len(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(caller_output, [0x5a; 16]);

        let empty_inputs = [
            input(GENERATED_INPUT_BYTES, b"".as_ptr().cast(), 0),
            input(
                GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
                ptr::dangling::<*const TypeBridgeProjectedValue>().cast(),
                0,
            ),
        ];
        let disjoint_output = [output(caller_output.as_mut_ptr().cast(), 8)];
        assert_eq!(
            // SAFETY: zero-count descriptors intentionally carry non-NULL
            // sentinel pointers, matching the established empty-view contract.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    empty_inputs.as_ptr(),
                    empty_inputs.len(),
                    disjoint_output.as_ptr(),
                    disjoint_output.len(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(caller_output, [0x5a; 16]);

        let ignored_empty_descriptor_pointer = input(GENERATED_INPUT_BYTES, ptr::null(), 0);
        let output = [output(caller_output.as_mut_ptr().cast(), 8)];
        assert_eq!(
            // SAFETY: top-level zero count ignores a non-NULL descriptor pointer.
            unsafe {
                type_bridge_generated_opaque_alias_preflight_v1(
                    &ignored_empty_descriptor_pointer,
                    0,
                    output.as_ptr(),
                    output.len(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(caller_output, [0x5a; 16]);
    }
}
