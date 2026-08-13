//! Provider-neutral immutable typed-query construction and direct execution.

#![allow(missing_docs)]

use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::projection::ProjectedTokenIdentity;
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::schema::{OwnsFactId, encode_declared_schema};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::{
    BindingHandle, FieldHandle, FunctionArgumentHandle, FunctionCallHandle, FunctionHandle,
    FunctionValueHandle, MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, MAX_QUERY_COLLECTION_MEMBERS,
    MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS, MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS,
    MAX_QUERY_TIMEOUT_MILLISECONDS, MatchMode, MissingOrder, OrderHandle, PredicateHandle,
    ProjectedQueryMaterializationLimits, ProjectedQueryOrigin, ProjectedQueryResult,
    ProjectedQuerySlotValue, ProjectedQueryValue, ProjectedReducedValue, ProjectedReductionGroup,
    ProjectedReductionRow, QueryExecutionDeadline, QueryExecutionResourceLimits, QueryHandle,
    Reduction, RemoteModelQueryV2Error, RoleHandle, RowCardinality, SelectionHandle, SessionHandle,
    SortDirection, ValidatedMatchRequest, Window, lower_execution_error,
    materialize_projected_query_result_with_budget, prepare_remote_model_query_v2_with_budget,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, close_box,
    guarded,
};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted, try_box, try_reserve};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::projected_model::{
    TypeBridgeProjectedThing, check_projected_attribute_value_ranges, check_projected_thing_ranges,
};
use crate::projected_token::{
    TypeBridgeProjectedTokenV1, resolve_field_token, resolve_function_token, resolve_model_token,
    resolve_role_token,
};
use crate::projected_value::TypeBridgeProjectedValue;
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    poisoned_transaction_diagnostic,
};

const DESCRIPTOR_VERSION: u32 = 1;
const SELECTED_SLOT_MAX: usize = 16;
const ORDER_TERM_MAX: usize = 16;
const BOOLEAN_TERM_MAX: usize = 256;
const FUNCTION_ARGUMENT_HOSTED_OBJECT_BYTES_MAX: usize = 65_535;
const OUTPUT_NAME_BYTES_MAX: usize = 128;
const REMOTE_RESPONSE_SNAPSHOT_LIMIT: usize = MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1);

const MATCH_EXACT: u32 = 1;
const MATCH_SUBTYPES: u32 = 2;
const COMPARE_EQUAL: u32 = 1;
const COMPARE_NOT_EQUAL: u32 = 2;
const COMPARE_LESS_THAN: u32 = 3;
const COMPARE_LESS_THAN_OR_EQUAL: u32 = 4;
const COMPARE_GREATER_THAN: u32 = 5;
const COMPARE_GREATER_THAN_OR_EQUAL: u32 = 6;
const COMPARE_CONTAINS: u32 = 7;
const COMPARE_STARTS_WITH: u32 = 8;
const COMPARE_ENDS_WITH: u32 = 9;
const COMPARE_REGEX: u32 = 10;
const PREDICATE_AND: u32 = 1;
const PREDICATE_OR: u32 = 2;
const PREDICATE_NOT: u32 = 3;
const SORT_ASCENDING: u32 = 1;
const SORT_DESCENDING: u32 = 2;
const MISSING_REJECT: u32 = 1;
const MISSING_FIRST: u32 = 2;
const MISSING_LAST: u32 = 3;
const SELECTION_ONE: u32 = 1;
const SELECTION_COLLECT: u32 = 2;
const SHAPE_POSITIONAL: u32 = 1;
const SHAPE_NAMED: u32 = 2;
const TERMINAL_ROWS: u32 = 1;
const TERMINAL_PAGE: u32 = 2;
const TERMINAL_COUNT: u32 = 3;
const TERMINAL_EXISTS: u32 = 4;
const TERMINAL_REDUCE: u32 = 5;
const TERMINAL_REDUCE_FIELD: u32 = 6;
const TERMINAL_REDUCE_FIELDS: u32 = 7;
const TERMINAL_FIRST: u32 = 8;
const ROWS_EXACTLY_ONE: u32 = 1;
const ROWS_BOUNDED_MANY: u32 = 2;
const REDUCER_COUNT: u32 = 1;
const REDUCER_SUM: u32 = 2;
const REDUCER_MIN: u32 = 3;
const REDUCER_MAX: u32 = 4;
const REDUCER_MEAN: u32 = 5;
const REDUCER_MEDIAN: u32 = 6;
const REDUCER_STD: u32 = 7;
const RESULT_ROWS: u32 = 1;
const RESULT_PAGE: u32 = 2;
const RESULT_COUNT: u32 = 3;
const RESULT_REDUCTION: u32 = 4;
const RESULT_FIELD_REDUCTION: u32 = 5;
const RESULT_FIELD_TUPLE_REDUCTION: u32 = 6;
const RESULT_EXISTS: u32 = 7;
const GROUP_NONE: u32 = 0;
const GROUP_THING: u32 = 1;
const GROUP_FIELD: u32 = 2;
const GROUP_FIELDS: u32 = 3;
const REDUCED_COUNT: u32 = 1;
const REDUCED_LONG: u32 = 2;
const REDUCED_DOUBLE: u32 = 3;
const FUNCTION_ARGUMENT_BINDING: u32 = 1;
const FUNCTION_ARGUMENT_VALUE: u32 = 2;
const FUNCTION_ARGUMENT_CALL: u32 = 3;

/// Version-1 descriptor for one stable order term.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryOrderDescriptorV1 {
    pub struct_size: u32,
    pub version: u32,
    pub field: *const TypeBridgeQueryField,
    pub expected_field: *const TypeBridgeProjectedTokenV1,
    pub direction: u32,
    pub missing: u32,
    pub reserved0: u32,
    pub reserved: [u64; 4],
}

/// Version-1 descriptor for one selected binding.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQuerySelectionDescriptorV1 {
    pub struct_size: u32,
    pub version: u32,
    pub binding: *const TypeBridgeQueryBinding,
    pub expected_model: *const TypeBridgeProjectedTokenV1,
    pub expected_mode: u32,
    pub kind: u32,
    pub distinct: u8,
    pub reserved0: [u8; 7],
    pub orders: *const *const TypeBridgeQueryOrder,
    pub order_count: usize,
    pub reserved: [u64; 4],
}

/// Version-1 query-shape slot.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryShapeSlotV1 {
    pub struct_size: u32,
    pub version: u32,
    pub selection: *const TypeBridgeQuerySelection,
    pub expected_model: *const TypeBridgeProjectedTokenV1,
    pub expected_mode: u32,
    pub expected_kind: u32,
    pub reserved0: u32,
    pub name: TypeBridgeByteView,
    pub reserved: [u64; 4],
}

/// Version-1 positional or named query-shape descriptor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryDescriptorV1 {
    pub struct_size: u32,
    pub version: u32,
    pub shape_kind: u32,
    pub reserved0: u32,
    pub slots: *const TypeBridgeQueryShapeSlotV1,
    pub slot_count: usize,
    pub reserved: [u64; 4],
}

/// Version-1 typed reducer descriptor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryReducerV1 {
    pub struct_size: u32,
    pub version: u32,
    pub kind: u32,
    pub reserved0: u32,
    pub input: *const TypeBridgeQueryField,
    pub expected_field: *const TypeBridgeProjectedTokenV1,
    pub reserved: [u64; 4],
}

/// Version-1 exact generated field reference.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryFieldReferenceV1 {
    pub struct_size: u32,
    pub version: u32,
    pub field: *const TypeBridgeQueryField,
    pub expected_field: *const TypeBridgeProjectedTokenV1,
    pub reserved: [u64; 4],
}

/// Version-1 reusable terminal descriptor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryTerminalDescriptorV1 {
    pub struct_size: u32,
    pub version: u32,
    pub kind: u32,
    pub cardinality: u32,
    pub root: *const TypeBridgeQueryBinding,
    pub expected_root_model: *const TypeBridgeProjectedTokenV1,
    pub expected_root_mode: u32,
    pub reserved1: u32,
    pub orders: *const *const TypeBridgeQueryOrder,
    pub order_count: usize,
    pub offset: u64,
    pub limit: u64,
    pub include_total: u8,
    pub reserved0: [u8; 7],
    pub group_binding: *const TypeBridgeQueryBinding,
    pub expected_group_model: *const TypeBridgeProjectedTokenV1,
    pub expected_group_mode: u32,
    pub reserved2: u32,
    pub group_fields: *const TypeBridgeQueryFieldReferenceV1,
    pub group_field_count: usize,
    pub reducers: *const TypeBridgeQueryReducerV1,
    pub reducer_count: usize,
    pub reserved: [u64; 4],
}

/// Version-1 common tightened direct and remote generated-query limits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryExecutionLimitsV1 {
    pub struct_size: u32,
    pub version: u32,
    pub timeout_milliseconds: u64,
    pub items: u64,
    pub bytes: u64,
    pub graph_nodes: u64,
    pub attribute_values: u64,
    pub collection_members: u64,
    pub role_players: u64,
    pub statements: u32,
    pub reserved0: u32,
    pub reserved: [u64; 4],
}

/// Version-1 page metadata view.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryPageMetadataV1 {
    pub struct_size: u32,
    pub version: u32,
    pub offset: u64,
    pub limit: u64,
    pub has_total: u8,
    pub reserved0: [u8; 7],
    pub total: u64,
    pub reserved: [u64; 4],
}

/// Version-1 reduced-value kind and optionality metadata.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryReducedValueMetadataV1 {
    pub struct_size: u32,
    pub version: u32,
    pub kind: u32,
    pub present: u8,
    pub reserved0: [u8; 3],
    pub reserved: [u64; 4],
}

/// Version-1 ABI-common generated schema-function argument witness.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryFunctionArgumentV1 {
    pub struct_size: u32,
    pub version: u32,
    pub kind: u32,
    pub reserved0: u32,
    pub binding: *const TypeBridgeQueryBinding,
    pub value: *const TypeBridgeQueryFunctionValue,
    pub call: *const TypeBridgeQueryFunctionCall,
    pub reserved: [u64; 4],
}

/// Version-1 positional offset in a generated function argument object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryFunctionArgumentMemberV1 {
    pub struct_size: u32,
    pub version: u32,
    pub args_offset: usize,
    pub reserved: [u64; 4],
}

/// Version-1 common prefix of every generated per-function arguments struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryFunctionArgumentsHeaderV1 {
    pub struct_size: u32,
    pub version: u32,
    pub reserved: [u64; 4],
}

/// Version-1 generated schema-function argument object and static member map.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeQueryFunctionArgumentsGraphV1 {
    pub struct_size: u32,
    pub version: u32,
    pub args: *const c_void,
    pub args_size: usize,
    pub members: *const TypeBridgeQueryFunctionArgumentMemberV1,
    pub member_count: usize,
    pub reserved: [u64; 3],
}

struct QuerySessionState {
    package: Arc<SchemaPackageState>,
    registry: Arc<DescriptorRegistry>,
    handle: SessionHandle,
}

/// Opaque immutable query-construction session.
pub struct TypeBridgeQuerySession {
    state: Arc<QuerySessionState>,
}

/// Opaque one-occurrence exact or subtype-inclusive binding.
pub struct TypeBridgeQueryBinding {
    state: Arc<QuerySessionState>,
    handle: BindingHandle,
    model: TypeId,
    mode: MatchMode,
}

/// Opaque descriptor-qualified scalar field bound to one occurrence.
pub struct TypeBridgeQueryField {
    state: Arc<QuerySessionState>,
    handle: FieldHandle,
    owner: TypeId,
    field: OwnsFactId,
}

/// Opaque descriptor-qualified relation role bound to one occurrence.
pub struct TypeBridgeQueryRole {
    state: Arc<QuerySessionState>,
    handle: RoleHandle,
    owner: TypeId,
    role: type_bridge_contract::id::RoleId,
}

/// Opaque exact generated scalar schema-function token.
pub struct TypeBridgeQueryFunction {
    state: Arc<QuerySessionState>,
    handle: FunctionHandle,
    function: FunctionId,
}

/// Opaque session-branded projected scalar admitted as a function argument.
pub struct TypeBridgeQueryFunctionValue {
    state: Arc<QuerySessionState>,
    handle: FunctionValueHandle,
}

/// Opaque immutable scalar schema-function call expression.
pub struct TypeBridgeQueryFunctionCall {
    state: Arc<QuerySessionState>,
    handle: FunctionCallHandle,
}

/// Opaque immutable typed predicate tree.
pub struct TypeBridgeQueryPredicate {
    state: Arc<QuerySessionState>,
    handle: PredicateHandle,
}

/// Opaque immutable stable order term.
pub struct TypeBridgeQueryOrder {
    state: Arc<QuerySessionState>,
    handle: OrderHandle,
}

/// Opaque immutable selected output slot.
pub struct TypeBridgeQuerySelection {
    state: Arc<QuerySessionState>,
    handle: SelectionHandle,
    model: TypeId,
    mode: MatchMode,
    kind: u32,
}

/// Opaque immutable query lineage.
pub struct TypeBridgeQuery {
    state: Arc<QuerySessionState>,
    handle: QueryHandle,
}

#[derive(Clone, Debug)]
struct ReducerSpec {
    reduction: Reduction,
    input: Option<TypeBridgeQueryFieldSpec>,
}

#[derive(Clone, Debug)]
struct TypeBridgeQueryFieldSpec {
    handle: FieldHandle,
}

#[derive(Clone, Debug)]
enum TerminalSpec {
    Rows {
        orders: Vec<OrderHandle>,
        window: Window,
        cardinality: RowCardinality,
    },
    Page {
        root: BindingHandle,
        orders: Vec<OrderHandle>,
        window: Window,
        include_total: bool,
    },
    Count {
        root: BindingHandle,
    },
    Exists {
        root: BindingHandle,
    },
    Reduce {
        root: BindingHandle,
        group: Option<BindingHandle>,
        reducers: Vec<ReducerSpec>,
    },
    ReduceField {
        root: BindingHandle,
        group: TypeBridgeQueryFieldSpec,
        reducers: Vec<ReducerSpec>,
    },
    ReduceFields {
        root: BindingHandle,
        groups: Vec<TypeBridgeQueryFieldSpec>,
        reducers: Vec<ReducerSpec>,
    },
}

/// Opaque immutable reusable terminal plan.
#[derive(Clone)]
pub struct TypeBridgeQueryTerminal {
    state: Arc<QuerySessionState>,
    query: QueryHandle,
    kind: u32,
    spec: TerminalSpec,
}

/// Opaque all-or-nothing materialized query result.
pub struct TypeBridgeQueryResult {
    state: Arc<QuerySessionState>,
    value: ProjectedQueryResult,
}

struct QueryRemoteContextState {
    package: Arc<SchemaPackageState>,
    registry: Arc<DescriptorRegistry>,
    authority: Arc<QueryAuthority>,
    advertisement: Vec<u8>,
    limits: QueryExecutionResourceLimits,
}

/// Opaque immutable authenticated caller-transport remote query context.
pub struct TypeBridgeQueryRemoteContext {
    state: Arc<QueryRemoteContextState>,
}

/// Opaque prepared one-exchange query request with a one-shot reply claim.
pub struct TypeBridgeQueryRemotePending {
    state: Arc<QueryRemoteContextState>,
    terminal: TypeBridgeQueryTerminal,
    pending: type_bridge_orm::PendingRemoteModelQueryV2,
    deadline: QueryExecutionDeadline,
    claim_consumed: AtomicBool,
}

/// Opaque one-shot claimed remote reply decoder.
pub struct TypeBridgeQueryRemoteClaim {
    state: Arc<QueryRemoteContextState>,
    terminal: TypeBridgeQueryTerminal,
    claimed: Mutex<Option<type_bridge_orm::ClaimedRemoteModelReplyV2>>,
    response_snapshot_limit: usize,
    deadline: QueryExecutionDeadline,
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C query diagnostic code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C query diagnostic message is valid")
}

fn invalid(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn integrity(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(code(code_value), message(message_value))
}

fn layout_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_query_descriptor_layout_invalid",
        "The typed-query descriptor layout or inactive lane is not canonical",
    )
}

fn nominal_mismatch() -> SdkExecutionDiagnostic {
    invalid(
        "c_query_nominal_contract_mismatch",
        "A typed-query handle does not match its generated nominal contract",
    )
}

fn package_mismatch() -> SdkExecutionDiagnostic {
    integrity(
        "c_query_package_lineage_mismatch",
        "Typed-query execution requires the exact retained schema-package lineage",
    )
}

fn result_mismatch() -> SdkExecutionDiagnostic {
    integrity(
        "c_query_result_contract_mismatch",
        "The typed-query result does not match the requested generated result contract",
    )
}

fn result_index_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_query_result_index_invalid",
        "The typed-query result index is outside its validated result shape",
    )
}

fn inactive_transaction() -> SdkExecutionDiagnostic {
    invalid(
        "c_query_read_transaction_inactive",
        "Typed-query execution requires one active read transaction",
    )
}

fn lower(error: type_bridge_orm::OrmError) -> SdkExecutionDiagnostic {
    lower_execution_error(error, SdkProviderOperation::Read)
}

fn lower_remote(error: RemoteModelQueryV2Error) -> SdkExecutionDiagnostic {
    match error {
        RemoteModelQueryV2Error::Match(error) => type_bridge_orm::lower_match_error(&error),
        RemoteModelQueryV2Error::Diagnostic(error) => {
            type_bridge_orm::lower_remote_query_diagnostic(error)
        }
    }
}

fn same_state(left: &Arc<QuerySessionState>, right: &Arc<QuerySessionState>) -> bool {
    Arc::ptr_eq(left, right)
}

fn match_mode(value: u32) -> Option<MatchMode> {
    match value {
        MATCH_EXACT => Some(MatchMode::Exact),
        MATCH_SUBTYPES => Some(MatchMode::Subtypes),
        _ => None,
    }
}

fn comparison(value: u32) -> Option<type_bridge_orm::ComparisonOp> {
    match value {
        COMPARE_EQUAL => Some(type_bridge_orm::ComparisonOp::Equal),
        COMPARE_NOT_EQUAL => Some(type_bridge_orm::ComparisonOp::NotEqual),
        COMPARE_LESS_THAN => Some(type_bridge_orm::ComparisonOp::LessThan),
        COMPARE_LESS_THAN_OR_EQUAL => Some(type_bridge_orm::ComparisonOp::LessThanOrEqual),
        COMPARE_GREATER_THAN => Some(type_bridge_orm::ComparisonOp::GreaterThan),
        COMPARE_GREATER_THAN_OR_EQUAL => Some(type_bridge_orm::ComparisonOp::GreaterThanOrEqual),
        COMPARE_CONTAINS => Some(type_bridge_orm::ComparisonOp::Contains),
        COMPARE_STARTS_WITH => Some(type_bridge_orm::ComparisonOp::StartsWith),
        COMPARE_ENDS_WITH => Some(type_bridge_orm::ComparisonOp::EndsWith),
        COMPARE_REGEX => Some(type_bridge_orm::ComparisonOp::Regex),
        _ => None,
    }
}

fn reduction(value: u32) -> Option<Reduction> {
    match value {
        REDUCER_COUNT => Some(Reduction::Count),
        REDUCER_SUM => Some(Reduction::Sum),
        REDUCER_MIN => Some(Reduction::Min),
        REDUCER_MAX => Some(Reduction::Max),
        REDUCER_MEAN => Some(Reduction::Mean),
        REDUCER_MEDIAN => Some(Reduction::Median),
        REDUCER_STD => Some(Reduction::Std),
        _ => None,
    }
}

fn descriptor_layout<T>(size: u32, version: u32, reserved: [u64; 4]) -> bool {
    size as usize == size_of::<T>() && version == DESCRIPTOR_VERSION && reserved == [0; 4]
}

fn collection_pointer_is_canonical<T>(pointer: *const T, count: usize, maximum: usize) -> bool {
    count <= maximum && ((count == 0 && pointer.is_null()) || (count != 0 && !pointer.is_null()))
}

fn preflight_outputs<T>(
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

unsafe fn initialize_scalar_output<T: Copy>(
    out_value: *mut T,
    default: T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: every non-null output was proven writable and disjoint by preflight.
    if !out_value.is_null() {
        unsafe { out_value.write_unaligned(default) };
    }
    if !out_diagnostics.is_null() {
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_value.is_null() || out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(())
}

fn check_state_ranges(
    state: &QuerySessionState,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_package_borrowed_ranges(&state.package)
}

macro_rules! impl_query_ranges {
    ($($type:ty),+ $(,)?) => {$ (
        impl $type {
            pub(crate) fn check_borrowed_ranges(
                &self,
                preflight: &DirectOutputPreflight,
            ) -> Result<(), TypeBridgeStatus> {
                check_state_ranges(&self.state, preflight)
            }
        }
    )+ };
}

impl_query_ranges!(
    TypeBridgeQuerySession,
    TypeBridgeQueryBinding,
    TypeBridgeQueryField,
    TypeBridgeQueryRole,
    TypeBridgeQueryFunction,
    TypeBridgeQueryFunctionValue,
    TypeBridgeQueryFunctionCall,
    TypeBridgeQueryPredicate,
    TypeBridgeQueryOrder,
    TypeBridgeQuerySelection,
    TypeBridgeQuery,
    TypeBridgeQueryTerminal,
);

impl TypeBridgeQueryRemoteContext {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.state.package)?;
        preflight.check_bytes(
            self.state.advertisement.as_ptr().cast(),
            self.state.advertisement.len(),
        )
    }
}

impl TypeBridgeQueryRemotePending {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.state.package)?;
        preflight.check_bytes(
            self.state.advertisement.as_ptr().cast(),
            self.state.advertisement.len(),
        )?;
        preflight.check_bytes(
            self.pending.request_bytes().as_ptr().cast(),
            self.pending.request_bytes().len(),
        )
    }
}

impl TypeBridgeQueryRemoteClaim {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.state.package)?;
        preflight.check_bytes(
            self.state.advertisement.as_ptr().cast(),
            self.state.advertisement.len(),
        )
    }
}

impl TypeBridgeQueryResult {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        check_state_ranges(&self.state, preflight)?;
        match self.value.value() {
            ProjectedQueryValue::Rows { rows }
            | ProjectedQueryValue::Page { entries: rows, .. } => {
                for row in rows {
                    for slot in row.slots() {
                        match slot.value() {
                            ProjectedQuerySlotValue::One(value) => {
                                check_projected_thing_ranges(value, preflight)?;
                            }
                            ProjectedQuerySlotValue::Many(values) => {
                                for value in values {
                                    check_projected_thing_ranges(value, preflight)?;
                                }
                            }
                        }
                    }
                }
            }
            ProjectedQueryValue::Reduction { rows, .. }
            | ProjectedQueryValue::FieldReduction { rows, .. }
            | ProjectedQueryValue::FieldTupleReduction { rows, .. } => {
                for row in rows {
                    match row.group() {
                        Some(ProjectedReductionGroup::Thing(value)) => {
                            check_projected_thing_ranges(value, preflight)?;
                        }
                        Some(ProjectedReductionGroup::Field(value)) => {
                            check_projected_attribute_value_ranges(value, preflight)?;
                        }
                        Some(ProjectedReductionGroup::Fields(values)) => {
                            for value in values {
                                check_projected_attribute_value_ranges(value, preflight)?;
                            }
                        }
                        None => {}
                    }
                }
            }
            ProjectedQueryValue::Count { .. } | ProjectedQueryValue::Exists { .. } => {}
        }
        Ok(())
    }
}

unsafe fn preflight_handle<T>(
    preflight: &DirectOutputPreflight,
    pointer: *const T,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        return Ok(());
    }
    preflight.check_bytes(pointer.cast(), size_of::<T>())
}

unsafe fn resolve_expected_model(
    state: &QuerySessionState,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<TypeId, SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is caller-readable for this call.
    unsafe { resolve_model_token(&state.package, token) }
}

unsafe fn require_binding_contract(
    binding: &TypeBridgeQueryBinding,
    token: *const TypeBridgeProjectedTokenV1,
    mode: u32,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: token resolution is against the exact state retained by the handle.
    let expected = unsafe { resolve_expected_model(&binding.state, token) }?;
    let Some(mode) = match_mode(mode) else {
        return Err(layout_invalid());
    };
    if expected != binding.model || mode != binding.mode {
        return Err(nominal_mismatch());
    }
    Ok(())
}

unsafe fn require_field_contract(
    field: &TypeBridgeQueryField,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is caller-readable for this call.
    let (owner, expected) = unsafe { resolve_field_token(&field.state.package, token) }?;
    if owner != field.owner || expected != field.field {
        return Err(nominal_mismatch());
    }
    Ok(())
}

unsafe fn require_role_contract(
    role: &TypeBridgeQueryRole,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is caller-readable for this call.
    let (owner, expected) = unsafe { resolve_role_token(&role.state.package, token) }?;
    if owner != role.owner || expected != role.role {
        return Err(nominal_mismatch());
    }
    Ok(())
}

unsafe fn require_function_contract(
    function: &TypeBridgeQueryFunction,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is caller-readable for this call.
    let expected = unsafe { resolve_function_token(&function.state.package, token) }?;
    if expected != function.function {
        return Err(nominal_mismatch());
    }
    Ok(())
}

fn field_target_name<'a>(
    state: &'a QuerySessionState,
    owner: &TypeId,
    field: &OwnsFactId,
) -> Option<&'a str> {
    state
        .package
        ._projection
        .models()
        .get(owner)?
        .query_tokens()
        .fields()
        .get(field)
        .map(|value| value.target_name().as_str())
}

fn type_descriptor_name(id: &TypeId) -> &str {
    id.label().as_str()
}

fn write_handle<T>(
    site: AllocationSite,
    value: T,
    out_value: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    match try_box(site, value) {
        Ok(value) => {
            // SAFETY: the common initializer proved this caller slot writable.
            unsafe { out_value.write_unaligned(Box::into_raw(value)) };
            TypeBridgeStatus::Ok
        }
        Err(_) => return_execution_error(allocation_exhausted(), out_diagnostics),
    }
}

fn query_result_kind(value: &ProjectedQueryValue) -> u32 {
    match value {
        ProjectedQueryValue::Rows { .. } => RESULT_ROWS,
        ProjectedQueryValue::Page { .. } => RESULT_PAGE,
        ProjectedQueryValue::Count { .. } => RESULT_COUNT,
        ProjectedQueryValue::Reduction { .. } => RESULT_REDUCTION,
        ProjectedQueryValue::FieldReduction { .. } => RESULT_FIELD_REDUCTION,
        ProjectedQueryValue::FieldTupleReduction { .. } => RESULT_FIELD_TUPLE_REDUCTION,
        ProjectedQueryValue::Exists { .. } => RESULT_EXISTS,
    }
}

/// Open an isolated provider-free typed-query construction session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_session_open(
    package: *const TypeBridgeSchemaPackage,
    out_session: *mut *mut TypeBridgeQuerySession,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_session, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: only the complete outer package object is checked before inspection.
    if let Err(status) = unsafe { preflight_handle(&preflight, package) } {
        return status;
    }
    if !package.is_null() {
        // SAFETY: the caller retains a live immutable package during the call.
        if let Err(status) = preflight.check_package_borrowed_ranges(unsafe { &*package }.state()) {
            return status;
        }
    }
    // SAFETY: preflight proved the two caller slots disjoint from inputs and each other.
    if let Err(status) = unsafe { initialize_execution_outputs(out_session, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable package through this call.
        let package = unsafe { &*package };
        let registry = match package.state().installed_projection.match_registry() {
            Ok(value) => Arc::new(value),
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let state = Arc::new(QuerySessionState {
            package: Arc::clone(package.state()),
            handle: SessionHandle::new(Arc::clone(&registry)),
            registry,
        });
        write_handle(
            AllocationSite::QuerySessionHandle,
            TypeBridgeQuerySession { state },
            out_session,
            out_diagnostics,
        )
    })
}

/// Close an immutable query-construction session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_session_close(
    session: *mut *mut TypeBridgeQuerySession,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(session) }
}

/// Open one fresh exact or subtype-inclusive model binding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_binding_open_v1(
    session: *const TypeBridgeQuerySession,
    model: *const TypeBridgeProjectedTokenV1,
    mode: u32,
    out_binding: *mut *mut TypeBridgeQueryBinding,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_binding, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: only complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, session) }, unsafe {
        preflight_handle(&preflight, model)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !session.is_null()
        // SAFETY: the complete immutable session object was checked above.
        && let Err(status) = unsafe { &*session }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full alias preflight completed before output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_binding, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if session.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(mode) = match_mode(mode) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        // SAFETY: caller retains the immutable session and token for the call.
        let session = unsafe { &*session };
        // SAFETY: token storage was checked and remains readable.
        let model = match unsafe { resolve_expected_model(&session.state, model) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        if !matches!(model.kind(), TypeKind::Entity | TypeKind::Relation) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match session
            .state
            .handle
            .binding(type_descriptor_name(&model), mode)
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryBindingHandle,
            TypeBridgeQueryBinding {
                state: Arc::clone(&session.state),
                handle,
                model,
                mode,
            },
            out_binding,
            out_diagnostics,
        )
    })
}

/// Close one immutable query binding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_binding_close(
    binding: *mut *mut TypeBridgeQueryBinding,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(binding) }
}

/// Resolve one generated field token against a binding occurrence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_open(
    binding: *const TypeBridgeQueryBinding,
    field: *const TypeBridgeProjectedTokenV1,
    out_field: *mut *mut TypeBridgeQueryField,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_field, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, binding) }, unsafe {
        preflight_handle(&preflight, field)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !binding.is_null()
        // SAFETY: caller retains one immutable binding object.
        && let Err(status) = unsafe { &*binding }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: alias preflight completed before independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_field, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if binding.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable binding and generated token.
        let binding = unsafe { &*binding };
        // SAFETY: generated token storage remains readable.
        let (owner, field_id) = match unsafe { resolve_field_token(&binding.state.package, field) }
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let Some(target_name) = field_target_name(&binding.state, &owner, &field_id) else {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        };
        let handle = match binding
            .handle
            .field_owned_by(type_descriptor_name(&owner), target_name)
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryFieldHandle,
            TypeBridgeQueryField {
                state: Arc::clone(&binding.state),
                handle,
                owner,
                field: field_id,
            },
            out_field,
            out_diagnostics,
        )
    })
}

/// Close one immutable query field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_close(
    field: *mut *mut TypeBridgeQueryField,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(field) }
}

/// Resolve one generated role token against a relation binding occurrence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_role_open(
    binding: *const TypeBridgeQueryBinding,
    role: *const TypeBridgeProjectedTokenV1,
    out_role: *mut *mut TypeBridgeQueryRole,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_role, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, binding) }, unsafe {
        preflight_handle(&preflight, role)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !binding.is_null()
        // SAFETY: caller retains one immutable binding object.
        && let Err(status) = unsafe { &*binding }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: alias preflight completed before output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_role, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if binding.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable binding and generated token.
        let binding = unsafe { &*binding };
        // SAFETY: generated token storage remains readable.
        let (owner, role_id) = match unsafe { resolve_role_token(&binding.state.package, role) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let handle = match binding
            .handle
            .role_owned_by(type_descriptor_name(&owner), role_id.label().as_str())
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryRoleHandle,
            TypeBridgeQueryRole {
                state: Arc::clone(&binding.state),
                handle,
                owner,
                role: role_id,
            },
            out_role,
            out_diagnostics,
        )
    })
}

/// Close one immutable query role.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_role_close(
    role: *mut *mut TypeBridgeQueryRole,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(role) }
}

/// Resolve one exact generated schema-function token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_open(
    session: *const TypeBridgeQuerySession,
    function: *const TypeBridgeProjectedTokenV1,
    out_function: *mut *mut TypeBridgeQueryFunction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_function, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, session) }, unsafe {
        preflight_handle(&preflight, function)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !session.is_null()
        // SAFETY: caller retains one immutable session object.
        && let Err(status) = unsafe { &*session }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_function, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if session.is_null() || function.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable session and generated token.
        let session = unsafe { &*session };
        // SAFETY: token storage was checked and remains readable.
        let function_id = match unsafe { resolve_function_token(&session.state.package, function) }
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let handle = match session.state.handle.function(&function_id) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryFunctionHandle,
            TypeBridgeQueryFunction {
                state: Arc::clone(&session.state),
                handle,
                function: function_id,
            },
            out_function,
            out_diagnostics,
        )
    })
}

/// Close one exact generated schema-function handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_close(
    function: *mut *mut TypeBridgeQueryFunction,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(function) }
}

/// Admit one exact-package projected scalar as a function input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_value_open(
    session: *const TypeBridgeQuerySession,
    value: *const TypeBridgeProjectedValue,
    out_value: *mut *mut TypeBridgeQueryFunctionValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_value, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, session) }, unsafe {
        preflight_handle(&preflight, value)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !session.is_null()
        // SAFETY: caller retains one immutable session object.
        && let Err(status) = unsafe { &*session }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !value.is_null()
        // SAFETY: caller retains one immutable projected scalar graph.
        && let Err(status) = unsafe { &*value }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if session.is_null() || value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through this call.
        let session = unsafe { &*session };
        // SAFETY: projected-value storage was fully preflighted above.
        let value = unsafe { &*value };
        if !Arc::ptr_eq(&session.state.package, value.package()) {
            return return_execution_error(package_mismatch(), out_diagnostics);
        }
        let handle = match session.state.handle.function_value(value.value()) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryFunctionValueHandle,
            TypeBridgeQueryFunctionValue {
                state: Arc::clone(&session.state),
                handle,
            },
            out_value,
            out_diagnostics,
        )
    })
}

/// Close one session-branded schema-function value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_value_close(
    value: *mut *mut TypeBridgeQueryFunctionValue,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(value) }
}

fn function_argument_member_layout(value: TypeBridgeQueryFunctionArgumentMemberV1) -> bool {
    descriptor_layout::<TypeBridgeQueryFunctionArgumentMemberV1>(
        value.struct_size,
        value.version,
        value.reserved,
    )
}

fn function_argument_layout(value: TypeBridgeQueryFunctionArgumentV1) -> bool {
    if !descriptor_layout::<TypeBridgeQueryFunctionArgumentV1>(
        value.struct_size,
        value.version,
        value.reserved,
    ) || value.reserved0 != 0
    {
        return false;
    }
    match value.kind {
        FUNCTION_ARGUMENT_BINDING => value.value.is_null() && value.call.is_null(),
        FUNCTION_ARGUMENT_VALUE => value.binding.is_null() && value.call.is_null(),
        FUNCTION_ARGUMENT_CALL => value.binding.is_null() && value.value.is_null(),
        _ => false,
    }
}

fn function_argument_graph_layout(value: TypeBridgeQueryFunctionArgumentsGraphV1) -> bool {
    if value.struct_size as usize != size_of::<TypeBridgeQueryFunctionArgumentsGraphV1>()
        || value.version != DESCRIPTOR_VERSION
        || value.reserved != [0; 3]
        || value.args_size > FUNCTION_ARGUMENT_HOSTED_OBJECT_BYTES_MAX
    {
        return false;
    }
    !value.args.is_null()
        && value.args_size >= size_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>()
        && (value.member_count == 0) == value.members.is_null()
}

/// Construct one immutable, signature-checked scalar schema-function call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_call_open_v1(
    function: *const TypeBridgeQueryFunction,
    expected_function: *const TypeBridgeProjectedTokenV1,
    arguments: *const TypeBridgeQueryFunctionArgumentsGraphV1,
    out_call: *mut *mut TypeBridgeQueryFunctionCall,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_call, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, function) },
        unsafe { preflight_handle(&preflight, expected_function) },
        unsafe { preflight_handle(&preflight, arguments) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !function.is_null()
        // SAFETY: caller retains one immutable function handle.
        && let Err(status) = unsafe { &*function }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    let snapshot = if arguments.is_null() {
        None
    } else {
        // SAFETY: the complete top-level graph descriptor was fenced above.
        Some(unsafe { arguments.read_unaligned() })
    };
    let graph_layout_valid = snapshot.is_some_and(function_argument_graph_layout);
    let traversal_limit_exceeded = snapshot.is_some_and(|value| {
        value.member_count > BOOLEAN_TERM_MAX
            || value.args_size > FUNCTION_ARGUMENT_HOSTED_OBJECT_BYTES_MAX
    });
    // An over-cap graph cannot be traversed safely. This safety ceiling is
    // therefore a read-only status with no output initialization/diagnostic.
    if traversal_limit_exceeded {
        return TypeBridgeStatus::ResourceLimit;
    }
    let mut all_members_valid = graph_layout_valid;
    if let Some(graph) = snapshot.filter(|_| all_members_valid) {
        if graph.args_size != 0
            && let Err(status) = preflight.check_bytes(graph.args, graph.args_size)
        {
            return status;
        }
        let Some(member_bytes) = graph
            .member_count
            .checked_mul(size_of::<TypeBridgeQueryFunctionArgumentMemberV1>())
        else {
            return TypeBridgeStatus::ResourceLimit;
        };
        if member_bytes != 0
            && let Err(status) = preflight.check_bytes(graph.members.cast(), member_bytes)
        {
            return status;
        }
        // SAFETY: the complete bounded generated argument object was fenced.
        let header = unsafe {
            graph
                .args
                .cast::<TypeBridgeQueryFunctionArgumentsHeaderV1>()
                .read_unaligned()
        };
        all_members_valid &= header.struct_size as usize == graph.args_size
            && header.version == DESCRIPTOR_VERSION
            && header.reserved == [0; 4];
        let mut previous_end = size_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>();
        for index in 0..graph.member_count {
            // SAFETY: the complete bounded static member table was fenced above.
            let member = unsafe { graph.members.add(index).read_unaligned() };
            let end = member
                .args_offset
                .checked_add(size_of::<TypeBridgeQueryFunctionArgumentV1>());
            let offset_valid =
                end.is_some_and(|end| member.args_offset >= previous_end && end <= graph.args_size);
            if let Some(end) = end.filter(|_| offset_valid) {
                previous_end = end;
            }
            let member_valid = function_argument_member_layout(member) && offset_valid;
            all_members_valid &= member_valid;
            if !offset_valid {
                continue;
            }
            // SAFETY: the complete generated argument object and this common
            // ABI witness were fenced above. Nominal generated witness structs
            // have this exact layout and may be unaligned in a hostile object.
            let argument = unsafe {
                graph
                    .args
                    .cast::<u8>()
                    .add(member.args_offset)
                    .cast::<TypeBridgeQueryFunctionArgumentV1>()
                    .read_unaligned()
            };
            // Every non-null lane may be a live opaque object even if the tag
            // or inactive-lane spelling is malformed. Fence outer storage first.
            for result in [
                unsafe { preflight_handle(&preflight, argument.binding) },
                unsafe { preflight_handle(&preflight, argument.value) },
                unsafe { preflight_handle(&preflight, argument.call) },
            ] {
                if let Err(status) = result {
                    return status;
                }
            }
            let argument_valid = function_argument_layout(argument);
            all_members_valid &= member_valid && argument_valid;
            if !member_valid || !argument_valid {
                continue;
            }
            let deep = match argument.kind {
                FUNCTION_ARGUMENT_BINDING if !argument.binding.is_null() => {
                    // SAFETY: the active binding outer range was fenced above.
                    unsafe { &*argument.binding }.check_borrowed_ranges(&preflight)
                }
                FUNCTION_ARGUMENT_VALUE if !argument.value.is_null() => {
                    // SAFETY: the active value outer range was fenced above.
                    unsafe { &*argument.value }.check_borrowed_ranges(&preflight)
                }
                FUNCTION_ARGUMENT_CALL if !argument.call.is_null() => {
                    // SAFETY: the active call outer range was fenced above.
                    unsafe { &*argument.call }.check_borrowed_ranges(&preflight)
                }
                _ => {
                    all_members_valid = false;
                    Ok(())
                }
            };
            if let Err(status) = deep {
                return status;
            }
        }
    }
    // SAFETY: full descriptor/opaque graph preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_call, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if function.is_null() || expected_function.is_null() || !graph_layout_valid {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        if !all_members_valid {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        // SAFETY: caller retains the immutable function handle and token.
        let function = unsafe { &*function };
        // SAFETY: exact generated function token remains readable.
        if let Err(error) = unsafe { require_function_contract(function, expected_function) } {
            return return_execution_error(error, out_diagnostics);
        }
        let mut handles = Vec::<FunctionArgumentHandle>::new();
        let graph = snapshot.expect("a validated function argument graph is present");
        if try_reserve(
            &mut handles,
            graph.member_count,
            AllocationSite::QueryFunctionArguments,
        )
        .is_err()
        {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        for index in 0..graph.member_count {
            // SAFETY: the static member table and generated argument object
            // were completely preflighted and remain immutable for this call.
            let member = unsafe { graph.members.add(index).read_unaligned() };
            let argument = unsafe {
                graph
                    .args
                    .cast::<u8>()
                    .add(member.args_offset)
                    .cast::<TypeBridgeQueryFunctionArgumentV1>()
                    .read_unaligned()
            };
            let handle = match argument.kind {
                FUNCTION_ARGUMENT_BINDING => {
                    // SAFETY: active binding storage was fully preflighted above.
                    let value = unsafe { &*argument.binding };
                    if !same_state(&function.state, &value.state) {
                        return return_execution_error(nominal_mismatch(), out_diagnostics);
                    }
                    value.handle.function_argument()
                }
                FUNCTION_ARGUMENT_VALUE => {
                    // SAFETY: active value storage was fully preflighted above.
                    let value = unsafe { &*argument.value };
                    if !same_state(&function.state, &value.state) {
                        return return_execution_error(nominal_mismatch(), out_diagnostics);
                    }
                    value.handle.function_argument()
                }
                FUNCTION_ARGUMENT_CALL => {
                    // SAFETY: active call storage was fully preflighted above.
                    let value = unsafe { &*argument.call };
                    if !same_state(&function.state, &value.state) {
                        return return_execution_error(nominal_mismatch(), out_diagnostics);
                    }
                    value.handle.function_argument()
                }
                _ => unreachable!("argument layouts were validated before output mutation"),
            };
            handles.push(handle);
        }
        let handle = match function.handle.call(handles) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryFunctionCallHandle,
            TypeBridgeQueryFunctionCall {
                state: Arc::clone(&function.state),
                handle,
            },
            out_call,
            out_diagnostics,
        )
    })
}

/// Close one immutable scalar schema-function call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_call_close(
    call: *mut *mut TypeBridgeQueryFunctionCall,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(call) }
}

/// Compare one scalar function result with one exact generated field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_call_compare_field(
    call: *const TypeBridgeQueryFunctionCall,
    comparison_kind: u32,
    field: *const TypeBridgeQueryField,
    expected_field: *const TypeBridgeProjectedTokenV1,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, call) },
        unsafe { preflight_handle(&preflight, field) },
        unsafe { preflight_handle(&preflight, expected_field) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !call.is_null()
        // SAFETY: caller retains one immutable function-call handle.
        && let Err(status) = unsafe { &*call }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !field.is_null()
        // SAFETY: caller retains one immutable field handle.
        && let Err(status) = unsafe { &*field }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full input-graph preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(operation) = comparison(comparison_kind) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        if call.is_null() || field.is_null() || expected_field.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: both immutable handles were fully preflighted above.
        let call = unsafe { &*call };
        // SAFETY: caller retains the immutable field and generated token.
        let field = unsafe { &*field };
        // SAFETY: exact generated field token remains readable.
        if let Err(error) = unsafe { require_field_contract(field, expected_field) } {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&call.state, &field.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match call.handle.compare_field(operation, &field.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&call.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Compare one scalar function result with one branded projected scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_call_compare_value(
    call: *const TypeBridgeQueryFunctionCall,
    comparison_kind: u32,
    value: *const TypeBridgeQueryFunctionValue,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, call) }, unsafe {
        preflight_handle(&preflight, value)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !call.is_null()
        // SAFETY: caller retains one immutable function-call handle.
        && let Err(status) = unsafe { &*call }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !value.is_null()
        // SAFETY: caller retains one immutable function-value handle.
        && let Err(status) = unsafe { &*value }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full input-graph preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(operation) = comparison(comparison_kind) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        if call.is_null() || value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through this call.
        let call = unsafe { &*call };
        // SAFETY: function-value storage was fully preflighted above.
        let value = unsafe { &*value };
        if !same_state(&call.state, &value.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match call.handle.compare_value(operation, &value.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&call.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Compare two scalar function results in the same query session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_function_call_compare_call(
    call: *const TypeBridgeQueryFunctionCall,
    comparison_kind: u32,
    other: *const TypeBridgeQueryFunctionCall,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, call) }, unsafe {
        preflight_handle(&preflight, other)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    for pointer in [call, other] {
        if !pointer.is_null()
            // SAFETY: caller retains both immutable function-call handles.
            && let Err(status) = unsafe { &*pointer }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
    }
    // SAFETY: full input-graph preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(operation) = comparison(comparison_kind) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        if call.is_null() || other.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through this call.
        let call = unsafe { &*call };
        // SAFETY: second call storage was fully preflighted above.
        let other = unsafe { &*other };
        if !same_state(&call.state, &other.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match call.handle.compare_call(operation, &other.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&call.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Compare one exact generated field with one scalar function result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_compare_function(
    field: *const TypeBridgeQueryField,
    expected_field: *const TypeBridgeProjectedTokenV1,
    comparison_kind: u32,
    call: *const TypeBridgeQueryFunctionCall,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, field) },
        unsafe { preflight_handle(&preflight, expected_field) },
        unsafe { preflight_handle(&preflight, call) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !field.is_null()
        // SAFETY: caller retains one immutable field handle.
        && let Err(status) = unsafe { &*field }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !call.is_null()
        // SAFETY: caller retains one immutable function-call handle.
        && let Err(status) = unsafe { &*call }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full input-graph preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(operation) = comparison(comparison_kind) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        if field.is_null() || expected_field.is_null() || call.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable field and exact generated token.
        let field = unsafe { &*field };
        // SAFETY: exact generated field token remains readable.
        if let Err(error) = unsafe { require_field_contract(field, expected_field) } {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: function-call storage was fully preflighted above.
        let call = unsafe { &*call };
        if !same_state(&field.state, &call.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match field.handle.compare_function(operation, &call.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&field.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

unsafe fn copied_iid(view: TypeBridgeByteView) -> Result<String, SdkExecutionDiagnostic> {
    if view.length == 0 || view.length > 258 || view.data.is_null() {
        return Err(invalid(
            "c_query_iid_view_invalid",
            "A typed-query IID must be one nonempty bounded canonical byte view",
        ));
    }
    // SAFETY: full bounded storage was checked by the caller before this snapshot.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.length) };
    let value = std::str::from_utf8(bytes).map_err(|_| {
        invalid(
            "c_query_iid_utf8_invalid",
            "A typed-query IID must be canonical UTF-8 identity text",
        )
    })?;
    Ok(value.to_owned())
}

unsafe fn binding_iid_impl(
    binding: *const TypeBridgeQueryBinding,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    iids: *const TypeBridgeByteView,
    iid_count: usize,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, binding) }, unsafe {
        preflight_handle(&preflight, expected_model)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !binding.is_null()
        // SAFETY: caller retains one immutable binding object.
        && let Err(status) = unsafe { &*binding }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !(1..=BOOLEAN_TERM_MAX).contains(&iid_count) || iids.is_null() {
        // SAFETY: outputs were fully preflighted and are initialized before semantic failure.
        if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) }
        {
            return status;
        }
        return return_execution_error(layout_invalid(), out_diagnostics);
    }
    let table_bytes = match iid_count.checked_mul(size_of::<TypeBridgeByteView>()) {
        Some(value) => value,
        None => return TypeBridgeStatus::ResourceLimit,
    };
    if let Err(status) = preflight.check_bytes(iids.cast(), table_bytes) {
        return status;
    }
    for index in 0..iid_count {
        // SAFETY: complete bounded view storage was checked immediately above.
        let view = unsafe { iids.add(index).read_unaligned() };
        if view.length != 0
            && let Err(status) = preflight.check_bytes(view.data.cast(), view.length)
        {
            return status;
        }
    }
    // SAFETY: full input graph alias preflight precedes all output writes.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if binding.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable binding and generated token.
        let binding = unsafe { &*binding };
        // SAFETY: generated nominal token is readable for this call.
        if let Err(error) =
            unsafe { require_binding_contract(binding, expected_model, expected_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        let mut copied = Vec::new();
        if try_reserve(&mut copied, iid_count, AllocationSite::QueryPredicateHandle).is_err() {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        for index in 0..iid_count {
            // SAFETY: complete view storage and its bytes were checked before output writes.
            let view = unsafe { iids.add(index).read_unaligned() };
            // SAFETY: this view's complete bytes remain readable for the call.
            match unsafe { copied_iid(view) } {
                Ok(value) => copied.push(value),
                Err(error) => return return_execution_error(error, out_diagnostics),
            }
        }
        let handle = if copied.len() == 1 {
            binding.handle.iid(copied.pop().expect("one IID remains"))
        } else {
            binding.handle.iid_in(copied)
        };
        let handle = match handle {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&binding.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Construct one generated-model-fenced IID predicate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_binding_iid_v1(
    binding: *const TypeBridgeQueryBinding,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    iid: TypeBridgeByteView,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the shared implementation snapshots exactly one bounded view.
    unsafe {
        binding_iid_impl(
            binding,
            expected_model,
            expected_mode,
            &iid,
            1,
            out_predicate,
            out_diagnostics,
        )
    }
}

/// Construct one generated-model-fenced bounded IID-set predicate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_binding_iid_in_v1(
    binding: *const TypeBridgeQueryBinding,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    iids: *const TypeBridgeByteView,
    iid_count: usize,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the shared implementation checks and snapshots the bounded view table.
    unsafe {
        binding_iid_impl(
            binding,
            expected_model,
            expected_mode,
            iids,
            iid_count,
            out_predicate,
            out_diagnostics,
        )
    }
}

/// Compare a generated field with one exact-domain projected literal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_compare_value(
    field: *const TypeBridgeQueryField,
    expected_field: *const TypeBridgeProjectedTokenV1,
    operation: u32,
    value: *const TypeBridgeProjectedValue,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, field) },
        unsafe { preflight_handle(&preflight, expected_field) },
        unsafe { preflight_handle(&preflight, value) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !field.is_null()
        // SAFETY: caller retains one immutable field object.
        && let Err(status) = unsafe { &*field }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !value.is_null()
        // SAFETY: caller retains one immutable projected scalar object.
        && let Err(status) = unsafe { &*value }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete alias preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if field.is_null() || value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(operation) = comparison(operation) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        // SAFETY: caller retains both immutable handles and the generated token.
        let field = unsafe { &*field };
        // SAFETY: generated field token remains readable for this call.
        if let Err(error) = unsafe { require_field_contract(field, expected_field) } {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: caller retains the immutable projected scalar.
        let value = unsafe { &*value };
        if !Arc::ptr_eq(&field.state.package, value.package())
            || value.value().attribute_type().kind() != TypeKind::Attribute
            || value.value().attribute_type().label() != field.field.attribute().label()
        {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = field
            .handle
            .compare_value(operation, value.value().to_attribute_value());
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&field.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Compare two exact generated fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_compare_field(
    left: *const TypeBridgeQueryField,
    expected_left: *const TypeBridgeProjectedTokenV1,
    operation: u32,
    right: *const TypeBridgeQueryField,
    expected_right: *const TypeBridgeProjectedTokenV1,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, left) },
        unsafe { preflight_handle(&preflight, expected_left) },
        unsafe { preflight_handle(&preflight, right) },
        unsafe { preflight_handle(&preflight, expected_right) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    for pointer in [left, right] {
        if !pointer.is_null()
            // SAFETY: caller retains the immutable field object.
            && let Err(status) = unsafe { &*pointer }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
    }
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if left.is_null() || right.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(operation) = comparison(operation) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        // SAFETY: caller retains both immutable field handles and their tokens.
        let left = unsafe { &*left };
        // SAFETY: generated token remains readable.
        if let Err(error) = unsafe { require_field_contract(left, expected_left) } {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: caller retains the second immutable field handle.
        let right = unsafe { &*right };
        // SAFETY: generated token remains readable.
        if let Err(error) = unsafe { require_field_contract(right, expected_right) } {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&left.state, &right.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match left.handle.compare_field(operation, &right.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&left.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Construct one exact generated field-presence predicate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_field_presence(
    field: *const TypeBridgeQueryField,
    expected_field: *const TypeBridgeProjectedTokenV1,
    present: u8,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, field) }, unsafe {
        preflight_handle(&preflight, expected_field)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !field.is_null()
        // SAFETY: caller retains one immutable field object.
        && let Err(status) = unsafe { &*field }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if field.is_null() || present > 1 {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable field handle and token.
        let field = unsafe { &*field };
        // SAFETY: generated token remains readable.
        if let Err(error) = unsafe { require_field_contract(field, expected_field) } {
            return return_execution_error(error, out_diagnostics);
        }
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&field.state),
                handle: field.handle.presence(present != 0),
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Construct one exact generated role-player edge predicate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_role_connects(
    role: *const TypeBridgeQueryRole,
    expected_role: *const TypeBridgeProjectedTokenV1,
    player: *const TypeBridgeQueryBinding,
    expected_player_model: *const TypeBridgeProjectedTokenV1,
    expected_player_mode: u32,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, role) },
        unsafe { preflight_handle(&preflight, expected_role) },
        unsafe { preflight_handle(&preflight, player) },
        unsafe { preflight_handle(&preflight, expected_player_model) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !role.is_null()
        // SAFETY: caller retains one immutable role object.
        && let Err(status) = unsafe { &*role }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !player.is_null()
        // SAFETY: caller retains one immutable binding object.
        && let Err(status) = unsafe { &*player }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if role.is_null() || player.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles and generated tokens.
        let role = unsafe { &*role };
        // SAFETY: generated role token remains readable.
        if let Err(error) = unsafe { require_role_contract(role, expected_role) } {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: caller retains the immutable player binding.
        let player = unsafe { &*player };
        // SAFETY: generated model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(player, expected_player_model, expected_player_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&role.state, &player.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match role.handle.connects(&player.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&role.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Construct one finite generated-role reachability predicate.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn type_bridge_query_session_reachable(
    session: *const TypeBridgeQuerySession,
    relation: *const TypeBridgeProjectedTokenV1,
    role_from: *const TypeBridgeProjectedTokenV1,
    role_to: *const TypeBridgeProjectedTokenV1,
    source: *const TypeBridgeQueryBinding,
    expected_source_model: *const TypeBridgeProjectedTokenV1,
    expected_source_mode: u32,
    target: *const TypeBridgeQueryBinding,
    expected_target_model: *const TypeBridgeProjectedTokenV1,
    expected_target_mode: u32,
    min_depth: u8,
    max_depth: u8,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, session) },
        unsafe { preflight_handle(&preflight, relation) },
        unsafe { preflight_handle(&preflight, role_from) },
        unsafe { preflight_handle(&preflight, role_to) },
        unsafe { preflight_handle(&preflight, source) },
        unsafe { preflight_handle(&preflight, expected_source_model) },
        unsafe { preflight_handle(&preflight, target) },
        unsafe { preflight_handle(&preflight, expected_target_model) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !session.is_null()
        // SAFETY: caller retains one immutable session object.
        && let Err(status) = unsafe { &*session }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    for pointer in [source, target] {
        if !pointer.is_null()
            // SAFETY: caller retains each immutable binding object.
            && let Err(status) = unsafe { &*pointer }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
    }
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if session.is_null() || source.is_null() || target.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable session and all tokens.
        let session = unsafe { &*session };
        // SAFETY: generated model token remains readable.
        let relation = match unsafe { resolve_expected_model(&session.state, relation) } {
            Ok(value) if value.kind() == TypeKind::Relation => value,
            Ok(_) => return return_execution_error(nominal_mismatch(), out_diagnostics),
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        // SAFETY: generated role tokens remain readable.
        let (from_owner, from_role) =
            match unsafe { resolve_role_token(&session.state.package, role_from) } {
                Ok(value) => value,
                Err(error) => return return_execution_error(error, out_diagnostics),
            };
        // SAFETY: generated role token remains readable.
        let (to_owner, to_role) =
            match unsafe { resolve_role_token(&session.state.package, role_to) } {
                Ok(value) => value,
                Err(error) => return return_execution_error(error, out_diagnostics),
            };
        if from_owner != relation || to_owner != relation {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        // SAFETY: caller retains both immutable endpoint bindings.
        let source = unsafe { &*source };
        // SAFETY: generated endpoint model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(source, expected_source_model, expected_source_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: caller retains the second immutable endpoint binding.
        let target = unsafe { &*target };
        // SAFETY: generated endpoint model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(target, expected_target_model, expected_target_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&session.state, &source.state) || !same_state(&session.state, &target.state)
        {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match session.state.handle.reachable(
            type_descriptor_name(&relation),
            from_role.label().as_str(),
            to_role.label().as_str(),
            &source.handle,
            &target.handle,
            min_depth,
            max_depth,
        ) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&session.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Combine or negate immutable predicate trees.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_predicate_combine(
    operation: u32,
    left: *const TypeBridgeQueryPredicate,
    right: *const TypeBridgeQueryPredicate,
    out_predicate: *mut *mut TypeBridgeQueryPredicate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_predicate, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // Even malformed/inactive lanes may point at live opaque handles. Check
    // their complete outer storage against outputs without dereferencing them.
    for pointer in [left, right] {
        if !pointer.is_null()
            && let Err(status) =
                preflight.check_bytes(pointer.cast(), size_of::<TypeBridgeQueryPredicate>())
        {
            return status;
        }
    }
    if !matches!(operation, PREDICATE_AND | PREDICATE_OR | PREDICATE_NOT) {
        // SAFETY: outputs are disjoint and writable.
        if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) }
        {
            return status;
        }
        return return_execution_error(layout_invalid(), out_diagnostics);
    }
    // The inactive right lane of NOT must be canonically null and is never followed.
    if operation == PREDICATE_NOT && !right.is_null() {
        // SAFETY: outputs are disjoint and writable.
        if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) }
        {
            return status;
        }
        return return_execution_error(layout_invalid(), out_diagnostics);
    }
    // SAFETY: only active complete outer objects are checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, left) } {
        return status;
    }
    if operation != PREDICATE_NOT
        && let Err(status) = unsafe { preflight_handle(&preflight, right) }
    {
        return status;
    }
    if !left.is_null()
        // SAFETY: caller retains the immutable predicate.
        && let Err(status) = unsafe { &*left }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if operation != PREDICATE_NOT
        && !right.is_null()
        // SAFETY: caller retains the active immutable predicate.
        && let Err(status) = unsafe { &*right }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full active-input alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_predicate, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if left.is_null() || (operation != PREDICATE_NOT && right.is_null()) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the active immutable predicate handles.
        let left = unsafe { &*left };
        let handle = match operation {
            PREDICATE_NOT => left.handle.not(),
            PREDICATE_AND | PREDICATE_OR => {
                // SAFETY: right is required non-null for binary operations.
                let right = unsafe { &*right };
                if !same_state(&left.state, &right.state) {
                    return return_execution_error(nominal_mismatch(), out_diagnostics);
                }
                let value = if operation == PREDICATE_AND {
                    left.handle.and(&right.handle)
                } else {
                    left.handle.or(&right.handle)
                };
                match value {
                    Ok(value) => value,
                    Err(error) => return return_execution_error(lower(error), out_diagnostics),
                }
            }
            _ => unreachable!("operation validated before input traversal"),
        };
        write_handle(
            AllocationSite::QueryPredicateHandle,
            TypeBridgeQueryPredicate {
                state: Arc::clone(&left.state),
                handle,
            },
            out_predicate,
            out_diagnostics,
        )
    })
}

/// Close one immutable predicate tree.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_predicate_close(
    predicate: *mut *mut TypeBridgeQueryPredicate,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(predicate) }
}

/// Open one immutable exact-field stable order term.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_order_open_v1(
    descriptor: *const TypeBridgeQueryOrderDescriptorV1,
    out_order: *mut *mut TypeBridgeQueryOrder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_order, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: only the complete top-level descriptor is checked before snapshot.
    if let Err(status) = unsafe { preflight_handle(&preflight, descriptor) } {
        return status;
    }
    let snapshot = if descriptor.is_null() {
        None
    } else {
        // SAFETY: caller retains one complete descriptor for this call.
        Some(unsafe { descriptor.read_unaligned() })
    };
    if let Some(value) = snapshot
        && descriptor_layout::<TypeBridgeQueryOrderDescriptorV1>(
            value.struct_size,
            value.version,
            value.reserved,
        )
        && value.reserved0 == 0
    {
        // SAFETY: layout validation identifies both active complete pointees.
        for result in [
            unsafe { preflight_handle(&preflight, value.field) },
            unsafe { preflight_handle(&preflight, value.expected_field) },
        ] {
            if let Err(status) = result {
                return status;
            }
        }
        if !value.field.is_null()
            // SAFETY: caller retains the immutable active field handle.
            && let Err(status) = unsafe { &*value.field }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
    }
    // SAFETY: all consumed input ranges were checked before output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_order, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(descriptor) = snapshot else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if !descriptor_layout::<TypeBridgeQueryOrderDescriptorV1>(
            descriptor.struct_size,
            descriptor.version,
            descriptor.reserved,
        ) || descriptor.reserved0 != 0
            || descriptor.field.is_null()
            || descriptor.expected_field.is_null()
        {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        let direction = match descriptor.direction {
            SORT_ASCENDING => SortDirection::Ascending,
            SORT_DESCENDING => SortDirection::Descending,
            _ => return return_execution_error(layout_invalid(), out_diagnostics),
        };
        let missing = match descriptor.missing {
            MISSING_REJECT => MissingOrder::Reject,
            MISSING_FIRST => MissingOrder::First,
            MISSING_LAST => MissingOrder::Last,
            _ => return return_execution_error(layout_invalid(), out_diagnostics),
        };
        // SAFETY: caller retains the active field handle and token.
        let field = unsafe { &*descriptor.field };
        // SAFETY: exact generated field token remains readable.
        if let Err(error) = unsafe { require_field_contract(field, descriptor.expected_field) } {
            return return_execution_error(error, out_diagnostics);
        }
        write_handle(
            AllocationSite::QueryOrderHandle,
            TypeBridgeQueryOrder {
                state: Arc::clone(&field.state),
                handle: field.handle.order(direction, missing),
            },
            out_order,
            out_diagnostics,
        )
    })
}

/// Close one immutable order term.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_order_close(
    order: *mut *mut TypeBridgeQueryOrder,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(order) }
}

unsafe fn snapshot_orders(
    state: &Arc<QuerySessionState>,
    orders: *const *const TypeBridgeQueryOrder,
    count: usize,
) -> Result<Vec<OrderHandle>, SdkExecutionDiagnostic> {
    let mut output = Vec::new();
    try_reserve(&mut output, count, AllocationSite::QueryOrderHandle)
        .map_err(|_| allocation_exhausted())?;
    for index in 0..count {
        // SAFETY: the complete pointer table was checked before this bounded walk.
        let pointer = unsafe { orders.add(index).read_unaligned() };
        if pointer.is_null() {
            return Err(layout_invalid());
        }
        // SAFETY: caller retains every immutable active order handle.
        let order = unsafe { &*pointer };
        if !same_state(state, &order.state) {
            return Err(nominal_mismatch());
        }
        output.push(order.handle.clone());
    }
    Ok(output)
}

/// Open one singular or collection selected output slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_selection_open_v1(
    descriptor: *const TypeBridgeQuerySelectionDescriptorV1,
    out_selection: *mut *mut TypeBridgeQuerySelection,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_selection, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: only the complete top-level descriptor is checked before snapshot.
    if let Err(status) = unsafe { preflight_handle(&preflight, descriptor) } {
        return status;
    }
    let snapshot = if descriptor.is_null() {
        None
    } else {
        // SAFETY: caller retains one complete descriptor for this call.
        Some(unsafe { descriptor.read_unaligned() })
    };
    let layout_active = snapshot.is_some_and(|value| {
        descriptor_layout::<TypeBridgeQuerySelectionDescriptorV1>(
            value.struct_size,
            value.version,
            value.reserved,
        ) && value.reserved0 == [0; 7]
            && matches!(value.kind, SELECTION_ONE | SELECTION_COLLECT)
            && value.distinct <= 1
            && collection_pointer_is_canonical(value.orders, value.order_count, ORDER_TERM_MAX)
            && (value.kind != SELECTION_ONE || (value.distinct == 0 && value.order_count == 0))
    });
    if layout_active {
        let value = snapshot.expect("checked as present");
        // SAFETY: layout validation identifies all active complete pointees.
        for result in [
            unsafe { preflight_handle(&preflight, value.binding) },
            unsafe { preflight_handle(&preflight, value.expected_model) },
        ] {
            if let Err(status) = result {
                return status;
            }
        }
        if value.order_count != 0 {
            // SAFETY: pointer/count canonicality and hard ceiling were checked above.
            if let Err(status) =
                unsafe { preflight.check_pointer_array(value.orders, value.order_count) }
            {
                return status;
            }
        }
        if !value.binding.is_null()
            // SAFETY: caller retains the immutable active binding handle.
            && let Err(status) = unsafe { &*value.binding }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
        for index in 0..value.order_count {
            // SAFETY: the complete bounded pointer table was checked above.
            let pointer = unsafe { value.orders.add(index).read_unaligned() };
            if !pointer.is_null()
                // SAFETY: caller retains every immutable order handle.
                && let Err(status) = unsafe { &*pointer }.check_borrowed_ranges(&preflight)
            {
                return status;
            }
        }
    }
    // SAFETY: complete active input alias preflight precedes output writes.
    if let Err(status) = unsafe { initialize_execution_outputs(out_selection, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let Some(descriptor) = snapshot else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if !layout_active || descriptor.binding.is_null() || descriptor.expected_model.is_null() {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        // SAFETY: caller retains the immutable binding and generated token.
        let binding = unsafe { &*descriptor.binding };
        // SAFETY: generated model token remains readable.
        if let Err(error) = unsafe {
            require_binding_contract(binding, descriptor.expected_model, descriptor.expected_mode)
        } {
            return return_execution_error(error, out_diagnostics);
        }
        let mut handle = if descriptor.kind == SELECTION_ONE {
            binding.handle.one()
        } else {
            match binding.handle.collect().distinct(descriptor.distinct != 0) {
                Ok(value) => value,
                Err(error) => return return_execution_error(lower(error), out_diagnostics),
            }
        };
        // SAFETY: the pointer table was completely checked and remains immutable.
        let orders = match unsafe {
            snapshot_orders(&binding.state, descriptor.orders, descriptor.order_count)
        } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        for order in orders {
            handle = match handle.order_by(order) {
                Ok(value) => value,
                Err(error) => return return_execution_error(lower(error), out_diagnostics),
            };
        }
        write_handle(
            AllocationSite::QuerySelectionHandle,
            TypeBridgeQuerySelection {
                state: Arc::clone(&binding.state),
                handle,
                model: binding.model.clone(),
                mode: binding.mode,
                kind: descriptor.kind,
            },
            out_selection,
            out_diagnostics,
        )
    })
}

/// Close one immutable selected output slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_selection_close(
    selection: *mut *mut TypeBridgeQuerySelection,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(selection) }
}

/// Open one immutable positional or declaration-checked named query lineage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_open_v1(
    session: *const TypeBridgeQuerySession,
    descriptor: *const TypeBridgeQueryDescriptorV1,
    out_query: *mut *mut TypeBridgeQuery,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_query, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, session) }, unsafe {
        preflight_handle(&preflight, descriptor)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !session.is_null()
        // SAFETY: caller retains one immutable session object.
        && let Err(status) = unsafe { &*session }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    let snapshot = if descriptor.is_null() {
        None
    } else {
        // SAFETY: caller retains one complete top-level descriptor.
        Some(unsafe { descriptor.read_unaligned() })
    };
    let top_layout = snapshot.is_some_and(|value| {
        descriptor_layout::<TypeBridgeQueryDescriptorV1>(
            value.struct_size,
            value.version,
            value.reserved,
        ) && value.reserved0 == 0
            && matches!(value.shape_kind, SHAPE_POSITIONAL | SHAPE_NAMED)
            && (1..=SELECTED_SLOT_MAX).contains(&value.slot_count)
            && !value.slots.is_null()
    });
    let mut slots_valid = top_layout;
    if let Some(descriptor) = snapshot.filter(|_| top_layout) {
        let table_bytes = match descriptor
            .slot_count
            .checked_mul(size_of::<TypeBridgeQueryShapeSlotV1>())
        {
            Some(value) => value,
            None => return TypeBridgeStatus::ResourceLimit,
        };
        if let Err(status) = preflight.check_bytes(descriptor.slots.cast(), table_bytes) {
            return status;
        }
        for index in 0..descriptor.slot_count {
            // SAFETY: the complete bounded slot table was checked above.
            let slot = unsafe { descriptor.slots.add(index).read_unaligned() };
            let name_canonical = if descriptor.shape_kind == SHAPE_POSITIONAL {
                slot.name.length == 0 && slot.name.data.is_null()
            } else {
                (1..=OUTPUT_NAME_BYTES_MAX).contains(&slot.name.length) && !slot.name.data.is_null()
            };
            if !descriptor_layout::<TypeBridgeQueryShapeSlotV1>(
                slot.struct_size,
                slot.version,
                slot.reserved,
            ) || slot.reserved0 != 0
                || !matches!(slot.expected_kind, SELECTION_ONE | SELECTION_COLLECT)
                || !name_canonical
            {
                slots_valid = false;
                break;
            }
            // SAFETY: valid slot layout identifies its active complete pointees.
            for result in [
                unsafe { preflight_handle(&preflight, slot.selection) },
                unsafe { preflight_handle(&preflight, slot.expected_model) },
            ] {
                if let Err(status) = result {
                    return status;
                }
            }
            if !slot.selection.is_null()
                // SAFETY: caller retains the immutable selected-slot handle.
                && let Err(status) = unsafe { &*slot.selection }.check_borrowed_ranges(&preflight)
            {
                return status;
            }
            if slot.name.length != 0
                && let Err(status) = preflight.check_bytes(slot.name.data.cast(), slot.name.length)
            {
                return status;
            }
        }
    }
    // SAFETY: complete active graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_query, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if session.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(descriptor) = snapshot else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if !top_layout || !slots_valid {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        // SAFETY: caller retains the immutable session for the call.
        let session = unsafe { &*session };
        let mut selections = Vec::new();
        let mut declarations = Vec::new();
        let mut named = Vec::new();
        if try_reserve(
            &mut selections,
            descriptor.slot_count,
            AllocationSite::QueryHandle,
        )
        .is_err()
        {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        if descriptor.shape_kind == SHAPE_NAMED
            && (try_reserve(
                &mut declarations,
                descriptor.slot_count,
                AllocationSite::QueryHandle,
            )
            .is_err()
                || try_reserve(
                    &mut named,
                    descriptor.slot_count,
                    AllocationSite::QueryHandle,
                )
                .is_err())
        {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        for index in 0..descriptor.slot_count {
            // SAFETY: complete bounded table and active pointees were checked before writes.
            let slot = unsafe { descriptor.slots.add(index).read_unaligned() };
            if slot.selection.is_null() || slot.expected_model.is_null() {
                return return_execution_error(layout_invalid(), out_diagnostics);
            }
            // SAFETY: caller retains every immutable selection handle.
            let selection = unsafe { &*slot.selection };
            // SAFETY: generated model token remains readable.
            let expected =
                match unsafe { resolve_expected_model(&selection.state, slot.expected_model) } {
                    Ok(value) => value,
                    Err(error) => return return_execution_error(error, out_diagnostics),
                };
            let Some(expected_mode) = match_mode(slot.expected_mode) else {
                return return_execution_error(layout_invalid(), out_diagnostics);
            };
            if !same_state(&session.state, &selection.state)
                || expected != selection.model
                || expected_mode != selection.mode
                || slot.expected_kind != selection.kind
            {
                return return_execution_error(nominal_mismatch(), out_diagnostics);
            }
            if descriptor.shape_kind == SHAPE_POSITIONAL {
                selections.push(selection.handle.clone());
            } else {
                // SAFETY: the bounded UTF-8 name bytes remain readable for the call.
                let bytes = unsafe { std::slice::from_raw_parts(slot.name.data, slot.name.length) };
                let name = match std::str::from_utf8(bytes) {
                    Ok(value) => value.to_owned(),
                    Err(_) => return return_execution_error(layout_invalid(), out_diagnostics),
                };
                declarations.push((
                    name.clone(),
                    type_descriptor_name(&expected).to_owned(),
                    slot.expected_kind == SELECTION_COLLECT,
                ));
                named.push((name, selection.handle.clone()));
            }
        }
        let shape = if descriptor.shape_kind == SHAPE_POSITIONAL {
            session.state.handle.positional(selections)
        } else {
            session.state.handle.named_checked(declarations, named)
        };
        let shape = match shape {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let handle = match session.state.handle.query(shape) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryHandle,
            TypeBridgeQuery {
                state: Arc::clone(&session.state),
                handle,
            },
            out_query,
            out_diagnostics,
        )
    })
}

unsafe fn preflight_query_transition<T>(
    query: *const TypeBridgeQuery,
    other: *const T,
    token: *const TypeBridgeProjectedTokenV1,
    out_query: *mut *mut TypeBridgeQuery,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    other_ranges: impl FnOnce(&T, &DirectOutputPreflight) -> Result<(), TypeBridgeStatus>,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = preflight_outputs(out_query, out_diagnostics)?;
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, query) },
        unsafe { preflight_handle(&preflight, other) },
        unsafe { preflight_handle(&preflight, token) },
    ] {
        result?;
    }
    if !query.is_null() {
        // SAFETY: caller retains the immutable query handle.
        unsafe { &*query }.check_borrowed_ranges(&preflight)?;
    }
    if !other.is_null() {
        // SAFETY: caller retains the immutable active companion handle.
        other_ranges(unsafe { &*other }, &preflight)?;
    }
    Ok(preflight)
}

/// Add one generated-model-fenced hidden binding to an immutable query lineage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_add_hidden(
    query: *const TypeBridgeQuery,
    binding: *const TypeBridgeQueryBinding,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    out_query: *mut *mut TypeBridgeQuery,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared transition preflight checks every complete active input.
    let _preflight = match unsafe {
        preflight_query_transition(
            query,
            binding,
            expected_model,
            out_query,
            out_diagnostics,
            TypeBridgeQueryBinding::check_borrowed_ranges,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_query, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if query.is_null() || binding.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles and the generated token.
        let query = unsafe { &*query };
        // SAFETY: caller retains the immutable binding.
        let binding = unsafe { &*binding };
        // SAFETY: generated model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(binding, expected_model, expected_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&query.state, &binding.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match query.handle.add_hidden(binding.handle.clone()) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryHandle,
            TypeBridgeQuery {
                state: Arc::clone(&query.state),
                handle,
            },
            out_query,
            out_diagnostics,
        )
    })
}

/// Attach one immutable predicate by canonical conjunction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_where(
    query: *const TypeBridgeQuery,
    predicate: *const TypeBridgeQueryPredicate,
    out_query: *mut *mut TypeBridgeQuery,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared transition preflight checks every complete active input.
    let _preflight = match unsafe {
        preflight_query_transition(
            query,
            predicate,
            ptr::null(),
            out_query,
            out_diagnostics,
            TypeBridgeQueryPredicate::check_borrowed_ranges,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_query, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if query.is_null() || predicate.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles for the call.
        let query = unsafe { &*query };
        // SAFETY: caller retains the immutable predicate.
        let predicate = unsafe { &*predicate };
        if !same_state(&query.state, &predicate.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match query.handle.where_predicate(predicate.handle.clone()) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryHandle,
            TypeBridgeQuery {
                state: Arc::clone(&query.state),
                handle,
            },
            out_query,
            out_diagnostics,
        )
    })
}

/// Permit one generated-model-fenced topology-level cross join.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn type_bridge_query_allow_cross_join(
    query: *const TypeBridgeQuery,
    left: *const TypeBridgeQueryBinding,
    expected_left_model: *const TypeBridgeProjectedTokenV1,
    expected_left_mode: u32,
    right: *const TypeBridgeQueryBinding,
    expected_right_model: *const TypeBridgeProjectedTokenV1,
    expected_right_mode: u32,
    out_query: *mut *mut TypeBridgeQuery,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_query, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, query) },
        unsafe { preflight_handle(&preflight, left) },
        unsafe { preflight_handle(&preflight, expected_left_model) },
        unsafe { preflight_handle(&preflight, right) },
        unsafe { preflight_handle(&preflight, expected_right_model) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !query.is_null()
        // SAFETY: caller retains the immutable query.
        && let Err(status) = unsafe { &*query }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    for pointer in [left, right] {
        if !pointer.is_null()
            // SAFETY: caller retains each immutable binding.
            && let Err(status) = unsafe { &*pointer }.check_borrowed_ranges(&preflight)
        {
            return status;
        }
    }
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_query, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if query.is_null() || left.is_null() || right.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains all immutable handles and generated tokens.
        let query = unsafe { &*query };
        // SAFETY: caller retains the first immutable binding.
        let left = unsafe { &*left };
        // SAFETY: generated model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(left, expected_left_model, expected_left_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        // SAFETY: caller retains the second immutable binding.
        let right = unsafe { &*right };
        // SAFETY: generated model token remains readable.
        if let Err(error) =
            unsafe { require_binding_contract(right, expected_right_model, expected_right_mode) }
        {
            return return_execution_error(error, out_diagnostics);
        }
        if !same_state(&query.state, &left.state) || !same_state(&query.state, &right.state) {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let handle = match query.handle.allow_cross_join(&left.handle, &right.handle) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryHandle,
            TypeBridgeQuery {
                state: Arc::clone(&query.state),
                handle,
            },
            out_query,
            out_diagnostics,
        )
    })
}

/// Close one immutable query lineage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_close(
    query: *mut *mut TypeBridgeQuery,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(query) }
}

fn terminal_lanes_valid(value: &TypeBridgeQueryTerminalDescriptorV1) -> bool {
    if !descriptor_layout::<TypeBridgeQueryTerminalDescriptorV1>(
        value.struct_size,
        value.version,
        value.reserved,
    ) || value.reserved0 != [0; 7]
        || value.reserved1 != 0
        || value.reserved2 != 0
        || value.include_total > 1
        || !matches!(
            value.kind,
            TERMINAL_ROWS
                | TERMINAL_PAGE
                | TERMINAL_COUNT
                | TERMINAL_EXISTS
                | TERMINAL_REDUCE
                | TERMINAL_REDUCE_FIELD
                | TERMINAL_REDUCE_FIELDS
                | TERMINAL_FIRST
        )
    {
        return false;
    }
    let row_terminal = matches!(value.kind, TERMINAL_ROWS | TERMINAL_FIRST);
    let page_terminal = value.kind == TERMINAL_PAGE;
    let root_terminal = matches!(
        value.kind,
        TERMINAL_PAGE
            | TERMINAL_COUNT
            | TERMINAL_EXISTS
            | TERMINAL_REDUCE
            | TERMINAL_REDUCE_FIELD
            | TERMINAL_REDUCE_FIELDS
    );
    let reduction_terminal = matches!(
        value.kind,
        TERMINAL_REDUCE | TERMINAL_REDUCE_FIELD | TERMINAL_REDUCE_FIELDS
    );
    if row_terminal {
        if !value.root.is_null()
            || !value.expected_root_model.is_null()
            || value.expected_root_mode != 0
        {
            return false;
        }
    } else if root_terminal
        && (value.root.is_null()
            || value.expected_root_model.is_null()
            || match_mode(value.expected_root_mode).is_none())
    {
        return false;
    }
    if !matches!(value.kind, TERMINAL_ROWS | TERMINAL_FIRST | TERMINAL_PAGE)
        && (!value.orders.is_null() || value.order_count != 0)
    {
        return false;
    }
    if matches!(value.kind, TERMINAL_ROWS | TERMINAL_FIRST | TERMINAL_PAGE)
        && !collection_pointer_is_canonical(value.orders, value.order_count, ORDER_TERM_MAX)
    {
        return false;
    }
    match value.kind {
        TERMINAL_ROWS => {
            if !matches!(value.cardinality, ROWS_EXACTLY_ONE | ROWS_BOUNDED_MANY)
                || value.limit == 0
                || (value.cardinality == ROWS_EXACTLY_ONE
                    && (value.offset != 0 || value.limit != 1))
            {
                return false;
            }
        }
        TERMINAL_FIRST => {
            if value.cardinality != ROWS_BOUNDED_MANY || value.offset != 0 || value.limit != 1 {
                return false;
            }
        }
        TERMINAL_PAGE => {
            if value.cardinality != ROWS_BOUNDED_MANY || value.limit == 0 {
                return false;
            }
        }
        _ => {
            if value.cardinality != ROWS_BOUNDED_MANY
                || value.offset != 0
                || value.limit != 0
                || value.include_total != 0
            {
                return false;
            }
        }
    }
    if !page_terminal && value.include_total != 0 {
        return false;
    }
    match value.kind {
        TERMINAL_REDUCE => {
            let no_group = value.group_binding.is_null()
                && value.expected_group_model.is_null()
                && value.expected_group_mode == 0;
            let binding_group = !value.group_binding.is_null()
                && !value.expected_group_model.is_null()
                && match_mode(value.expected_group_mode).is_some();
            if !(no_group || binding_group)
                || !value.group_fields.is_null()
                || value.group_field_count != 0
            {
                return false;
            }
        }
        TERMINAL_REDUCE_FIELD => {
            if !value.group_binding.is_null()
                || !value.expected_group_model.is_null()
                || value.expected_group_mode != 0
                || value.group_fields.is_null()
                || value.group_field_count != 1
            {
                return false;
            }
        }
        TERMINAL_REDUCE_FIELDS => {
            if !value.group_binding.is_null()
                || !value.expected_group_model.is_null()
                || value.expected_group_mode != 0
                || value.group_fields.is_null()
                || !(2..=SELECTED_SLOT_MAX).contains(&value.group_field_count)
            {
                return false;
            }
        }
        _ => {
            if !value.group_binding.is_null()
                || !value.expected_group_model.is_null()
                || value.expected_group_mode != 0
                || !value.group_fields.is_null()
                || value.group_field_count != 0
            {
                return false;
            }
        }
    }
    if reduction_terminal {
        collection_pointer_is_canonical(value.reducers, value.reducer_count, SELECTED_SLOT_MAX)
            && value.reducer_count != 0
    } else {
        value.reducers.is_null() && value.reducer_count == 0
    }
}

fn field_reference_layout(value: &TypeBridgeQueryFieldReferenceV1) -> bool {
    descriptor_layout::<TypeBridgeQueryFieldReferenceV1>(
        value.struct_size,
        value.version,
        value.reserved,
    ) && !value.field.is_null()
        && !value.expected_field.is_null()
}

fn reducer_layout(value: &TypeBridgeQueryReducerV1) -> bool {
    if !descriptor_layout::<TypeBridgeQueryReducerV1>(
        value.struct_size,
        value.version,
        value.reserved,
    ) || value.reserved0 != 0
        || reduction(value.kind).is_none()
    {
        return false;
    }
    if value.kind == REDUCER_COUNT {
        value.input.is_null() && value.expected_field.is_null()
    } else {
        !value.input.is_null() && !value.expected_field.is_null()
    }
}

unsafe fn terminal_active_preflight(
    preflight: &DirectOutputPreflight,
    value: &TypeBridgeQueryTerminalDescriptorV1,
) -> Result<bool, TypeBridgeStatus> {
    if !terminal_lanes_valid(value) {
        return Ok(false);
    }
    // SAFETY: lane validation identifies required root and optional active group binding.
    for result in [
        unsafe { preflight_handle(preflight, value.root) },
        unsafe { preflight_handle(preflight, value.expected_root_model) },
        unsafe { preflight_handle(preflight, value.group_binding) },
        unsafe { preflight_handle(preflight, value.expected_group_model) },
    ] {
        result?;
    }
    if !value.root.is_null() {
        // SAFETY: caller retains the active immutable root binding.
        unsafe { &*value.root }.check_borrowed_ranges(preflight)?;
    }
    if !value.group_binding.is_null() {
        // SAFETY: caller retains the active immutable group binding.
        unsafe { &*value.group_binding }.check_borrowed_ranges(preflight)?;
    }
    if value.order_count != 0 {
        // SAFETY: lane validation bounded the canonical active pointer table.
        unsafe { preflight.check_pointer_array(value.orders, value.order_count) }?;
        for index in 0..value.order_count {
            // SAFETY: the complete bounded pointer table was checked above.
            let pointer = unsafe { value.orders.add(index).read_unaligned() };
            if !pointer.is_null() {
                // SAFETY: caller retains every active immutable order handle.
                unsafe { &*pointer }.check_borrowed_ranges(preflight)?;
            }
        }
    }
    if value.group_field_count != 0 {
        let bytes = value
            .group_field_count
            .checked_mul(size_of::<TypeBridgeQueryFieldReferenceV1>())
            .ok_or(TypeBridgeStatus::ResourceLimit)?;
        preflight.check_bytes(value.group_fields.cast(), bytes)?;
        for index in 0..value.group_field_count {
            // SAFETY: the complete bounded group-field table was checked above.
            let field = unsafe { value.group_fields.add(index).read_unaligned() };
            if !field_reference_layout(&field) {
                return Ok(false);
            }
            // SAFETY: nested layout validation identifies both active pointees.
            for result in [
                unsafe { preflight_handle(preflight, field.field) },
                unsafe { preflight_handle(preflight, field.expected_field) },
            ] {
                result?;
            }
            // SAFETY: caller retains the active immutable field handle.
            unsafe { &*field.field }.check_borrowed_ranges(preflight)?;
        }
    }
    if value.reducer_count != 0 {
        let bytes = value
            .reducer_count
            .checked_mul(size_of::<TypeBridgeQueryReducerV1>())
            .ok_or(TypeBridgeStatus::ResourceLimit)?;
        preflight.check_bytes(value.reducers.cast(), bytes)?;
        for index in 0..value.reducer_count {
            // SAFETY: the complete bounded reducer table was checked above.
            let reducer = unsafe { value.reducers.add(index).read_unaligned() };
            if !reducer_layout(&reducer) {
                return Ok(false);
            }
            if !reducer.input.is_null() {
                // SAFETY: nested layout validation identifies both active pointees.
                for result in [
                    unsafe { preflight_handle(preflight, reducer.input) },
                    unsafe { preflight_handle(preflight, reducer.expected_field) },
                ] {
                    result?;
                }
                // SAFETY: caller retains the active immutable input field.
                unsafe { &*reducer.input }.check_borrowed_ranges(preflight)?;
            }
        }
    }
    Ok(true)
}

unsafe fn field_spec(
    state: &Arc<QuerySessionState>,
    field: *const TypeBridgeQueryField,
    expected: *const TypeBridgeProjectedTokenV1,
) -> Result<TypeBridgeQueryFieldSpec, SdkExecutionDiagnostic> {
    if field.is_null() {
        return Err(layout_invalid());
    }
    // SAFETY: active field pointer was completely preflighted before output writes.
    let field = unsafe { &*field };
    // SAFETY: exact generated field token remains readable.
    unsafe { require_field_contract(field, expected) }?;
    if !same_state(state, &field.state) {
        return Err(nominal_mismatch());
    }
    Ok(TypeBridgeQueryFieldSpec {
        handle: field.handle.clone(),
    })
}

unsafe fn reducer_specs(
    state: &Arc<QuerySessionState>,
    values: *const TypeBridgeQueryReducerV1,
    count: usize,
) -> Result<Vec<ReducerSpec>, SdkExecutionDiagnostic> {
    let mut output = Vec::new();
    try_reserve(&mut output, count, AllocationSite::QueryTerminalHandle)
        .map_err(|_| allocation_exhausted())?;
    for index in 0..count {
        // SAFETY: complete bounded reducer table was preflighted before output writes.
        let value = unsafe { values.add(index).read_unaligned() };
        let reduction = reduction(value.kind).ok_or_else(layout_invalid)?;
        let input = if value.input.is_null() {
            None
        } else {
            // SAFETY: nested active field and token were completely preflighted.
            Some(unsafe { field_spec(state, value.input, value.expected_field) }?)
        };
        output.push(ReducerSpec { reduction, input });
    }
    Ok(output)
}

/// Open one immutable reusable terminal plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_terminal_open_v1(
    query: *const TypeBridgeQuery,
    descriptor: *const TypeBridgeQueryTerminalDescriptorV1,
    out_terminal: *mut *mut TypeBridgeQueryTerminal,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_terminal, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer query and descriptor objects are checked before dereference.
    for result in [unsafe { preflight_handle(&preflight, query) }, unsafe {
        preflight_handle(&preflight, descriptor)
    }] {
        if let Err(status) = result {
            return status;
        }
    }
    if !query.is_null()
        // SAFETY: caller retains the immutable query handle.
        && let Err(status) = unsafe { &*query }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    let snapshot = if descriptor.is_null() {
        None
    } else {
        // SAFETY: caller retains one complete top-level terminal descriptor.
        Some(unsafe { descriptor.read_unaligned() })
    };
    let active_valid = match snapshot {
        Some(value) => {
            // SAFETY: only validated active lanes are followed by this preflight.
            match unsafe { terminal_active_preflight(&preflight, &value) } {
                Ok(value) => value,
                Err(status) => return status,
            }
        }
        None => false,
    };
    // SAFETY: complete active input graph alias preflight precedes output writes.
    if let Err(status) = unsafe { initialize_execution_outputs(out_terminal, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if query.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(descriptor) = snapshot else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if !active_valid {
            return return_execution_error(layout_invalid(), out_diagnostics);
        }
        // SAFETY: caller retains the immutable query.
        let query = unsafe { &*query };
        let orders = if descriptor.order_count == 0 {
            Vec::new()
        } else {
            // SAFETY: active bounded pointer table and pointees were completely preflighted.
            match unsafe {
                snapshot_orders(&query.state, descriptor.orders, descriptor.order_count)
            } {
                Ok(value) => value,
                Err(error) => return return_execution_error(error, out_diagnostics),
            }
        };
        let root = if descriptor.root.is_null() {
            None
        } else {
            // SAFETY: active root binding and generated token were completely preflighted.
            let root = unsafe { &*descriptor.root };
            // SAFETY: generated model token remains readable.
            if let Err(error) = unsafe {
                require_binding_contract(
                    root,
                    descriptor.expected_root_model,
                    descriptor.expected_root_mode,
                )
            } {
                return return_execution_error(error, out_diagnostics);
            }
            if !same_state(&query.state, &root.state) {
                return return_execution_error(nominal_mismatch(), out_diagnostics);
            }
            Some(root.handle.clone())
        };
        let reducers = if descriptor.reducer_count == 0 {
            Vec::new()
        } else {
            // SAFETY: complete reducer graph was preflighted before output writes.
            match unsafe {
                reducer_specs(&query.state, descriptor.reducers, descriptor.reducer_count)
            } {
                Ok(value) => value,
                Err(error) => return return_execution_error(error, out_diagnostics),
            }
        };
        let spec = match descriptor.kind {
            TERMINAL_ROWS | TERMINAL_FIRST => TerminalSpec::Rows {
                orders,
                window: Window {
                    offset: descriptor.offset,
                    limit: descriptor.limit,
                },
                cardinality: if descriptor.cardinality == ROWS_EXACTLY_ONE {
                    RowCardinality::ExactlyOne
                } else {
                    RowCardinality::BoundedMany
                },
            },
            TERMINAL_PAGE => TerminalSpec::Page {
                root: root.expect("page root lane validated"),
                orders,
                window: Window {
                    offset: descriptor.offset,
                    limit: descriptor.limit,
                },
                include_total: descriptor.include_total != 0,
            },
            TERMINAL_COUNT => TerminalSpec::Count {
                root: root.expect("count root lane validated"),
            },
            TERMINAL_EXISTS => TerminalSpec::Exists {
                root: root.expect("exists root lane validated"),
            },
            TERMINAL_REDUCE => {
                let group = if descriptor.group_binding.is_null() {
                    None
                } else {
                    // SAFETY: optional active binding and token were completely preflighted.
                    let group = unsafe { &*descriptor.group_binding };
                    // SAFETY: exact generated model token remains readable.
                    if let Err(error) = unsafe {
                        require_binding_contract(
                            group,
                            descriptor.expected_group_model,
                            descriptor.expected_group_mode,
                        )
                    } {
                        return return_execution_error(error, out_diagnostics);
                    }
                    if !same_state(&query.state, &group.state) {
                        return return_execution_error(nominal_mismatch(), out_diagnostics);
                    }
                    Some(group.handle.clone())
                };
                TerminalSpec::Reduce {
                    root: root.expect("reduction root lane validated"),
                    group,
                    reducers,
                }
            }
            TERMINAL_REDUCE_FIELD => {
                // SAFETY: complete group-field reference table was preflighted.
                let group = unsafe { descriptor.group_fields.read_unaligned() };
                // SAFETY: active group field and token were completely preflighted.
                let group =
                    match unsafe { field_spec(&query.state, group.field, group.expected_field) } {
                        Ok(value) => value,
                        Err(error) => return return_execution_error(error, out_diagnostics),
                    };
                TerminalSpec::ReduceField {
                    root: root.expect("field reduction root lane validated"),
                    group,
                    reducers,
                }
            }
            TERMINAL_REDUCE_FIELDS => {
                let mut groups = Vec::new();
                if try_reserve(
                    &mut groups,
                    descriptor.group_field_count,
                    AllocationSite::QueryTerminalHandle,
                )
                .is_err()
                {
                    return return_execution_error(allocation_exhausted(), out_diagnostics);
                }
                for index in 0..descriptor.group_field_count {
                    // SAFETY: complete bounded reference table was preflighted.
                    let group = unsafe { descriptor.group_fields.add(index).read_unaligned() };
                    // SAFETY: active group field and token were completely preflighted.
                    match unsafe { field_spec(&query.state, group.field, group.expected_field) } {
                        Ok(value) => groups.push(value),
                        Err(error) => return return_execution_error(error, out_diagnostics),
                    }
                }
                TerminalSpec::ReduceFields {
                    root: root.expect("tuple reduction root lane validated"),
                    groups,
                    reducers,
                }
            }
            _ => unreachable!("terminal kind validated before active traversal"),
        };
        // Validate provider-free now, but discard the invocation-local proof.
        if let Err(error) = validate_terminal(&query.handle, &spec) {
            return return_execution_error(lower(error), out_diagnostics);
        }
        write_handle(
            AllocationSite::QueryTerminalHandle,
            TypeBridgeQueryTerminal {
                state: Arc::clone(&query.state),
                query: query.handle.clone(),
                kind: descriptor.kind,
                spec,
            },
            out_terminal,
            out_diagnostics,
        )
    })
}

fn validate_terminal(
    query: &QueryHandle,
    spec: &TerminalSpec,
) -> Result<ValidatedMatchRequest, type_bridge_orm::OrmError> {
    match spec {
        TerminalSpec::Rows {
            orders,
            window,
            cardinality,
        } => query.validate_fetch_rows(orders, *window, *cardinality),
        TerminalSpec::Page {
            root,
            orders,
            window,
            include_total,
        } => query.validate_page_by(root, orders, *window, *include_total),
        TerminalSpec::Count { root } => query.validate_count_by(root),
        TerminalSpec::Exists { root } => query.validate_exists_by(root),
        TerminalSpec::Reduce {
            root,
            group,
            reducers,
        } => {
            let terms = reducers
                .iter()
                .map(|value| {
                    (
                        value.reduction,
                        value.input.as_ref().map(|field| &field.handle),
                    )
                })
                .collect::<Vec<_>>();
            query.validate_reduce_by(root, group.as_ref(), &terms)
        }
        TerminalSpec::ReduceField {
            root,
            group,
            reducers,
        } => {
            let terms = reducers
                .iter()
                .map(|value| {
                    (
                        value.reduction,
                        value.input.as_ref().map(|field| &field.handle),
                    )
                })
                .collect::<Vec<_>>();
            query.validate_reduce_by_field(root, &group.handle, &terms)
        }
        TerminalSpec::ReduceFields {
            root,
            groups,
            reducers,
        } => {
            let group_refs = groups.iter().map(|value| &value.handle).collect::<Vec<_>>();
            let terms = reducers
                .iter()
                .map(|value| {
                    (
                        value.reduction,
                        value.input.as_ref().map(|field| &field.handle),
                    )
                })
                .collect::<Vec<_>>();
            query.validate_reduce_by_fields(root, &group_refs, &terms)
        }
    }
}

/// Close one immutable reusable terminal plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_terminal_close(
    terminal: *mut *mut TypeBridgeQueryTerminal,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(terminal) }
}

struct ParsedLimits {
    resources: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
}

struct ExecuteInvocationPreflight {
    outputs: DirectOutputPreflight,
    answer: type_bridge_orm::AnswerCancellation,
    parsed: Result<ParsedLimits, SdkExecutionDiagnostic>,
}

fn parse_common_limits(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
) -> Result<QueryExecutionResourceLimits, SdkExecutionDiagnostic> {
    let value = value.unwrap_or(TypeBridgeQueryExecutionLimitsV1 {
        struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
        version: DESCRIPTOR_VERSION,
        timeout_milliseconds: MAX_QUERY_TIMEOUT_MILLISECONDS,
        items: MAX_QUERY_ITEMS,
        bytes: MAX_QUERY_BYTES,
        graph_nodes: MAX_QUERY_GRAPH_NODES,
        attribute_values: MAX_QUERY_ATTRIBUTE_VALUES,
        collection_members: MAX_QUERY_COLLECTION_MEMBERS,
        role_players: MAX_QUERY_ROLE_PLAYERS,
        statements: MAX_QUERY_STATEMENTS,
        reserved0: 0,
        reserved: [0; 4],
    });
    if !descriptor_layout::<TypeBridgeQueryExecutionLimitsV1>(
        value.struct_size,
        value.version,
        value.reserved,
    ) || value.reserved0 != 0
    {
        return Err(invalid(
            "c_query_execution_limits_invalid",
            "Typed-query limits must use the canonical version-1 descriptor layout",
        ));
    }
    Ok(QueryExecutionResourceLimits::tightened(
        value.timeout_milliseconds,
        value.items,
        value.bytes,
        value.graph_nodes,
        value.attribute_values,
        value.collection_members,
        value.role_players,
        value.statements,
    ))
}

fn parse_limits(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
    cancellation: &type_bridge_orm::AnswerCancellation,
) -> Result<ParsedLimits, SdkExecutionDiagnostic> {
    let common = parse_common_limits(value)?;
    let deadline = QueryExecutionDeadline::for_limits(common);
    deadline.check(cancellation)?;
    Ok(ParsedLimits {
        resources: common,
        deadline,
    })
}

unsafe fn execute_preflight<T>(
    target: *const T,
    target_package: impl FnOnce(&T) -> &Arc<SchemaPackageState>,
    terminal: *const TypeBridgeQueryTerminal,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeQueryResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<ExecuteInvocationPreflight, TypeBridgeStatus> {
    let preflight = preflight_outputs(out_result, out_diagnostics)?;
    // SAFETY: complete outer objects are checked before dereference.
    for result in [
        unsafe { preflight_handle(&preflight, target) },
        unsafe { preflight_handle(&preflight, terminal) },
        unsafe { preflight_handle(&preflight, limits) },
        unsafe { preflight_handle(&preflight, cancellation) },
    ] {
        result?;
    }
    let limits_snapshot = if limits.is_null() {
        None
    } else {
        // SAFETY: the complete limits descriptor was fenced above.
        Some(unsafe { limits.read_unaligned() })
    };
    let answer = if cancellation.is_null() {
        type_bridge_orm::AnswerCancellation::default()
    } else {
        // SAFETY: the complete cancellation handle was fenced above and the
        // caller retains it immutable throughout this invocation.
        unsafe { &*cancellation }.answer_cancellation()
    };
    // Capture the one absolute invocation budget before any potentially large
    // terminal/package borrowed-range walk. Interruption errors are retained
    // until that read-only walk proves outputs cannot alias live input bytes.
    let parsed = parse_limits(limits_snapshot, &answer);
    #[cfg(test)]
    QUERY_PREFLIGHT_DELAY_MILLISECONDS.with(|delay| {
        let delay = delay.replace(0);
        if delay != 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
    });
    if !target.is_null() {
        // SAFETY: caller retains the complete immutable execution target.
        preflight.check_package_borrowed_ranges(unsafe { target_package(&*target) })?;
    }
    if !terminal.is_null() {
        // SAFETY: caller retains the immutable terminal plan.
        unsafe { &*terminal }.check_borrowed_ranges(&preflight)?;
    }
    Ok(ExecuteInvocationPreflight {
        outputs: preflight,
        answer,
        parsed,
    })
}

struct MaterializationBudget<'a> {
    limits: ProjectedQueryMaterializationLimits,
    cancellation: &'a type_bridge_orm::AnswerCancellation,
    deadline: QueryExecutionDeadline,
}

fn materialize_result(
    terminal: &TypeBridgeQueryTerminal,
    registry: &DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    request: ValidatedMatchRequest,
    result: type_bridge_orm::ValidatedMatchResult,
    budget: MaterializationBudget<'_>,
) -> Result<TypeBridgeQueryResult, SdkExecutionDiagnostic> {
    #[cfg(test)]
    MATERIALIZATION_PANIC.with(|armed| {
        if armed.replace(false) {
            panic!("injected C query materialization panic");
        }
    });
    materialize_projected_query_result_with_budget(
        &terminal.state.package.installed_projection,
        registry,
        origin,
        request,
        result,
        budget.limits,
        budget.cancellation,
        Some(budget.deadline),
    )
    .map(|value| TypeBridgeQueryResult {
        state: Arc::clone(&terminal.state),
        value,
    })
}

#[cfg(test)]
std::thread_local! {
    static MATERIALIZATION_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static QUERY_PREFLIGHT_DELAY_MILLISECONDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

enum QueryExecutionFailure {
    Diagnostic(SdkExecutionDiagnostic),
    Panic,
}

fn return_query_failure(
    failure: QueryExecutionFailure,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    match failure {
        QueryExecutionFailure::Diagnostic(value) => return_execution_error(value, out_diagnostics),
        QueryExecutionFailure::Panic => TypeBridgeStatus::Panic,
    }
}

fn remote_context_authority_mismatch() -> SdkExecutionDiagnostic {
    integrity(
        "c_query_remote_authority_projection_mismatch",
        "Remote typed-query authority does not match the retained generated projection",
    )
}

fn remote_claim_consumed() -> SdkExecutionDiagnostic {
    integrity(
        "c_query_remote_claim_consumed",
        "The one-shot remote typed-query reply claim was already consumed",
    )
}

fn remote_claim_state_unavailable() -> SdkExecutionDiagnostic {
    integrity(
        "c_query_remote_claim_state_unavailable",
        "The one-shot remote typed-query reply claim state is unavailable",
    )
}

fn remote_advertisement_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_query_remote_advertisement_view_invalid",
        "Remote typed-query capability advertisement bytes must be nonempty and canonical",
    )
}

fn remote_advertisement_limit() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        code("c_query_remote_advertisement_limit_exceeded"),
        message("Remote typed-query capability advertisement exceeds the wire ceiling"),
    )
}

fn remote_response_limit() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        code("c_query_remote_response_limit_exceeded"),
        message("Remote typed-query reply exceeds the authenticated snapshot ceiling"),
    )
}

unsafe fn copy_query_bytes(
    view: TypeBridgeByteView,
    site: AllocationSite,
) -> Result<Vec<u8>, crate::allocation::AllocationFailure> {
    let mut output = Vec::new();
    try_reserve(&mut output, view.length, site)?;
    if view.length != 0 {
        // SAFETY: the caller retains this already-preflighted byte range for the call.
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(view.data, view.length) });
    }
    Ok(output)
}

fn build_remote_context_state(
    package: &Arc<SchemaPackageState>,
    advertisement: Vec<u8>,
    limits: QueryExecutionResourceLimits,
) -> Result<QueryRemoteContextState, SdkExecutionDiagnostic> {
    RemoteCapabilities::decode(&advertisement)
        .map_err(type_bridge_orm::lower_remote_query_diagnostic)?;
    let declared = encode_declared_schema(package._authority.declared_schema())
        .map_err(type_bridge_orm::lower_remote_query_diagnostic)?;
    let authority = QueryAuthority::from_declared_bytes(
        &declared,
        package._authority.managed_scope().id().as_str(),
        package._authority.semantic_profile().id().as_str(),
    )
    .map_err(type_bridge_orm::lower_remote_query_diagnostic)?;
    if !authority.matches_semantic_fingerprint(
        package
            .installed_projection
            .projection()
            .semantic_fingerprint(),
    ) {
        return Err(remote_context_authority_mismatch());
    }
    let registry = package
        .installed_projection
        .match_registry()
        .map(Arc::new)
        .map_err(lower)?;
    Ok(QueryRemoteContextState {
        package: Arc::clone(package),
        registry,
        authority: Arc::new(authority),
        advertisement,
        limits,
    })
}

fn take_remote_claim(
    claim: &TypeBridgeQueryRemoteClaim,
) -> Result<type_bridge_orm::ClaimedRemoteModelReplyV2, SdkExecutionDiagnostic> {
    claim
        .claimed
        .lock()
        .map_err(|_| remote_claim_state_unavailable())?
        .take()
        .ok_or_else(remote_claim_consumed)
}

/// Execute one reusable terminal through a database-owned read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_query_execute_v1(
    database: *const TypeBridgeDatabase,
    terminal: *const TypeBridgeQueryTerminal,
    expected_terminal_kind: u32,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeQueryResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared execution preflight checks every complete active input.
    let invocation = match unsafe {
        execute_preflight(
            database,
            TypeBridgeDatabase::package_state,
            terminal,
            limits,
            cancellation,
            out_result,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    let ExecuteInvocationPreflight {
        outputs: _preflight,
        answer,
        parsed,
    } = invocation;
    // SAFETY: full alias preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if database.is_null() || terminal.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through the call.
        let database = unsafe { &*database };
        // SAFETY: caller retains the immutable terminal.
        let terminal = unsafe { &*terminal };
        let parsed = match parsed {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        if let Err(error) = parsed.deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        if terminal.kind != expected_terminal_kind {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        if !Arc::ptr_eq(database.package_state(), &terminal.state.package) {
            return return_execution_error(package_mismatch(), out_diagnostics);
        }
        let request = match validate_terminal(&terminal.query, &terminal.spec) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let value = database
                .block_on(
                    database.orm_database().execute_match_with_limits(
                        &terminal.state.registry,
                        &request,
                        parsed
                            .resources
                            .direct_with_deadline(answer.clone(), parsed.deadline),
                    ),
                )
                .map_err(lower)?;
            materialize_result(
                terminal,
                &terminal.state.registry,
                ProjectedQueryOrigin::for_database(database.orm_database()),
                request,
                value,
                MaterializationBudget {
                    limits: parsed.resources.projected(),
                    cancellation: &answer,
                    deadline: parsed.deadline,
                },
            )
        }));
        let result = match outcome {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                return return_query_failure(
                    QueryExecutionFailure::Diagnostic(error),
                    out_diagnostics,
                );
            }
            Err(_) => {
                return return_query_failure(QueryExecutionFailure::Panic, out_diagnostics);
            }
        };
        let result = reservation.initialize(result);
        // SAFETY: output slot was initialized and remains caller-writable.
        unsafe { out_result.write_unaligned(Box::into_raw(result)) };
        TypeBridgeStatus::Ok
    })
}

/// Execute one reusable terminal through a caller-owned borrowed read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_query_execute_v1(
    transaction: *const TypeBridgeReadTransaction,
    terminal: *const TypeBridgeQueryTerminal,
    expected_terminal_kind: u32,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeQueryResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared execution preflight checks every complete active input.
    let invocation = match unsafe {
        execute_preflight(
            transaction,
            TypeBridgeReadTransaction::package_state,
            terminal,
            limits,
            cancellation,
            out_result,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    let ExecuteInvocationPreflight {
        outputs: _preflight,
        answer,
        parsed,
    } = invocation;
    // SAFETY: full alias preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if transaction.is_null() || terminal.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through execution.
        let transaction = unsafe { &*transaction };
        // SAFETY: caller retains the immutable terminal.
        let terminal = unsafe { &*terminal };
        let parsed = match parsed {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        if let Err(error) = parsed.deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        if terminal.kind != expected_terminal_kind {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        if !Arc::ptr_eq(transaction.package_state(), &terminal.state.package) {
            return return_execution_error(package_mismatch(), out_diagnostics);
        }
        if transaction.is_poisoned() {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        let Some(context) = transaction.context() else {
            return return_execution_error(inactive_transaction(), out_diagnostics);
        };
        let request = match validate_terminal(&terminal.query, &terminal.spec) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let value = transaction
                .block_on(
                    context.execute_match_with_limits(
                        &terminal.state.registry,
                        &request,
                        parsed
                            .resources
                            .direct_with_deadline(answer.clone(), parsed.deadline),
                    ),
                )
                .map_err(lower)?;
            let origin = ProjectedQueryOrigin::for_transaction(context)?;
            materialize_result(
                terminal,
                &terminal.state.registry,
                origin,
                request,
                value,
                MaterializationBudget {
                    limits: parsed.resources.projected(),
                    cancellation: &answer,
                    deadline: parsed.deadline,
                },
            )
        }));
        let result = match outcome {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return return_execution_error(error, out_diagnostics),
            Err(_) => {
                transaction.mark_poisoned();
                return TypeBridgeStatus::Panic;
            }
        };
        let result = reservation.initialize(result);
        // SAFETY: output slot was initialized and remains caller-writable.
        unsafe { out_result.write_unaligned(Box::into_raw(result)) };
        TypeBridgeStatus::Ok
    })
}

/// Open one managed caller-transport remote query context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_context_open_v1(
    package: *const TypeBridgeSchemaPackage,
    advertisement: TypeBridgeByteView,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    out_context: *mut *mut TypeBridgeQueryRemoteContext,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_context, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, package) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, limits) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !package.is_null()
        // SAFETY: caller retains one complete immutable package handle.
        && let Err(status) = preflight
            .check_package_borrowed_ranges(unsafe { &*package }.state())
    {
        return status;
    }
    if !advertisement.data.is_null()
        && advertisement.length != 0
        && let Err(status) = preflight.check_bytes(advertisement.data.cast(), advertisement.length)
    {
        return status;
    }
    let limits_snapshot = if limits.is_null() {
        None
    } else {
        // SAFETY: the complete limits descriptor was preflighted before this copy.
        Some(unsafe { limits.read_unaligned() })
    };
    // SAFETY: complete graph preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_context, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        if advertisement.data.is_null() || advertisement.length == 0 {
            return return_execution_error(remote_advertisement_invalid(), out_diagnostics);
        }
        if advertisement.length > MAX_REMOTE_ENVELOPE_BYTES {
            return return_execution_error(remote_advertisement_limit(), out_diagnostics);
        }
        let common = match parse_common_limits(limits_snapshot) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryRemoteContextHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: the advertisement range was preflighted and remains immutable for the call.
        let advertisement = match unsafe {
            copy_query_bytes(advertisement, AllocationSite::QueryRemoteAdvertisementBytes)
        } {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: caller retains the immutable package for the call.
        let package = unsafe { &*package };
        let state = match build_remote_context_state(package.state(), advertisement, common) {
            Ok(value) => Arc::new(value),
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let context = reservation.initialize(TypeBridgeQueryRemoteContext { state });
        // SAFETY: the output slot was preflighted and initialized.
        unsafe { out_context.write_unaligned(Box::into_raw(context)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one immutable caller-transport remote query context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_context_close(
    context: *mut *mut TypeBridgeQueryRemoteContext,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(context) }
}

/// Prepare one fresh authenticated remote request from a reusable terminal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_prepare_v1(
    context: *const TypeBridgeQueryRemoteContext,
    terminal: *const TypeBridgeQueryTerminal,
    expected_terminal_kind: u32,
    cancellation: *const TypeBridgeCancellation,
    out_pending: *mut *mut TypeBridgeQueryRemotePending,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_pending, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, context) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, terminal) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, cancellation) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    let answer = if cancellation.is_null() {
        type_bridge_orm::AnswerCancellation::default()
    } else {
        // SAFETY: the complete cancellation handle was fenced above and the
        // caller retains it immutable throughout preparation.
        unsafe { &*cancellation }.answer_cancellation()
    };
    let invocation_budget = if context.is_null() {
        None
    } else {
        // SAFETY: the complete context outer object was fenced above.
        let deadline = QueryExecutionDeadline::for_limits(unsafe { &*context }.state.limits);
        let check = deadline.check(&answer);
        Some((deadline, check))
    };
    #[cfg(test)]
    QUERY_PREFLIGHT_DELAY_MILLISECONDS.with(|delay| {
        let delay = delay.replace(0);
        if delay != 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
    });
    if !context.is_null()
        // SAFETY: caller retains the immutable context handle.
        && let Err(status) = unsafe { &*context }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !terminal.is_null()
        // SAFETY: caller retains the immutable terminal handle.
        && let Err(status) = unsafe { &*terminal }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete graph preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_pending, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if context.is_null() || terminal.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles for the call.
        let context = unsafe { &*context };
        // SAFETY: caller retains both immutable handles for the call.
        let terminal = unsafe { &*terminal };
        let (deadline, interruption) = invocation_budget.expect("non-null context has a budget");
        if let Err(error) = interruption {
            return return_execution_error(error, out_diagnostics);
        }
        if let Err(error) = deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        if terminal.kind != expected_terminal_kind {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        if !Arc::ptr_eq(&context.state.package, &terminal.state.package) {
            return return_execution_error(package_mismatch(), out_diagnostics);
        }
        let request = match validate_terminal(&terminal.query, &terminal.spec) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryRemotePendingHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let pending = match prepare_remote_model_query_v2_with_budget(
            &context.state.authority,
            &context.state.registry,
            request,
            &context.state.advertisement,
            context.state.limits.remote(),
            deadline,
            &answer,
        ) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower_remote(error), out_diagnostics),
        };
        if let Err(error) = deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        let pending = reservation.initialize(TypeBridgeQueryRemotePending {
            state: Arc::clone(&context.state),
            terminal: terminal.clone(),
            pending,
            deadline,
            claim_consumed: AtomicBool::new(false),
        });
        // SAFETY: the output slot was preflighted and initialized.
        unsafe { out_pending.write_unaligned(Box::into_raw(pending)) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow the exact canonical request bytes owned by one pending request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_pending_request_bytes(
    pending: *const TypeBridgeQueryRemotePending,
    out_request: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    let preflight =
        match direct_output_preflight(&[(out_request.cast(), size_of::<TypeBridgeByteView>())]) {
            Ok(value) => value,
            Err(status) => return status,
        };
    // SAFETY: only the complete outer object is checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, pending) } {
        return status;
    }
    if !pending.is_null()
        // SAFETY: caller retains the immutable pending request handle.
        && let Err(status) = unsafe { &*pending }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete graph preflight proved the view slot writable.
    if let Err(status) = unsafe { crate::abi::initialize_view(out_request) } {
        return status;
    }
    guarded(|| {
        if pending.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable pending request handle.
        let request = unsafe { &*pending }.pending.request_bytes();
        // SAFETY: the initialized output slot remains caller-writable.
        unsafe {
            out_request.write_unaligned(TypeBridgeByteView {
                data: request.as_ptr(),
                length: request.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Atomically claim the sole authenticated reply slot for a pending request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_pending_claim(
    pending: *const TypeBridgeQueryRemotePending,
    cancellation: *const TypeBridgeCancellation,
    out_claim: *mut *mut TypeBridgeQueryRemoteClaim,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_claim, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, pending) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, cancellation) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !pending.is_null()
        // SAFETY: caller retains the immutable pending request handle.
        && let Err(status) = unsafe { &*pending }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: complete graph preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_claim, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if pending.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let answer = if cancellation.is_null() {
            type_bridge_orm::AnswerCancellation::default()
        } else {
            // SAFETY: caller retains the immutable cancellation handle through claim.
            unsafe { &*cancellation }.answer_cancellation()
        };
        // SAFETY: caller retains the immutable pending request handle.
        let pending = unsafe { &*pending };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryRemoteClaimHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        if pending
            .claim_consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return return_execution_error(remote_claim_consumed(), out_diagnostics);
        }
        if let Err(error) = pending.deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        let claimed = match pending.pending.claim_reply_with_cancellation(&answer) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower_remote(error), out_diagnostics),
        };
        if let Err(error) = pending.deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        let response_snapshot_limit = claimed.response_snapshot_limit();
        debug_assert_eq!(response_snapshot_limit, REMOTE_RESPONSE_SNAPSHOT_LIMIT);
        let claim = reservation.initialize(TypeBridgeQueryRemoteClaim {
            state: Arc::clone(&pending.state),
            terminal: pending.terminal.clone(),
            claimed: Mutex::new(Some(claimed)),
            response_snapshot_limit,
            deadline: pending.deadline,
        });
        // SAFETY: the output slot was preflighted and initialized.
        unsafe { out_claim.write_unaligned(Box::into_raw(claim)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one prepared pending request handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_pending_close(
    pending: *mut *mut TypeBridgeQueryRemotePending,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(pending) }
}

/// Return the maximum reply snapshot admitted for one pending exchange.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_pending_response_snapshot_limit(
    pending: *const TypeBridgeQueryRemotePending,
    out_limit: *mut usize,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[(out_limit.cast(), size_of::<usize>())]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: only the complete outer object is checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, pending) } {
        return status;
    }
    if !pending.is_null()
        // SAFETY: caller retains the immutable pending handle.
        && let Err(status) = unsafe { &*pending }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    if !out_limit.is_null() {
        // SAFETY: preflight proved this non-null scalar output writable.
        unsafe { out_limit.write_unaligned(0) };
    }
    if out_limit.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        if pending.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // The protocol admits authenticated failure evidence up to the hard
        // envelope ceiling even when the caller tightens successful bytes to
        // zero. One additional byte preserves the exact over-limit verdict.
        unsafe { out_limit.write_unaligned(REMOTE_RESPONSE_SNAPSHOT_LIMIT) };
        TypeBridgeStatus::Ok
    })
}

/// Snapshot, authenticate, decode, and materialize one claimed remote reply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_claim_decode_v1(
    claim: *const TypeBridgeQueryRemoteClaim,
    expected_terminal_kind: u32,
    cancellation: *const TypeBridgeCancellation,
    response: TypeBridgeByteView,
    out_result: *mut *mut TypeBridgeQueryResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_result, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, claim) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, cancellation) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !claim.is_null()
        // SAFETY: caller retains the immutable claim handle.
        && let Err(status) = unsafe { &*claim }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    let response_view_canonical = (response.length == 0 && response.data.is_null())
        || (response.length != 0 && !response.data.is_null());
    if response.length != 0
        && !response.data.is_null()
        && let Err(status) = preflight.check_bytes(response.data.cast(), response.length)
    {
        return status;
    }
    // SAFETY: complete graph and active response-range preflight precede initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    if !response_view_canonical {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        if claim.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable claim handle.
        let claim = unsafe { &*claim };
        if claim.terminal.kind != expected_terminal_kind {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        let answer = if cancellation.is_null() {
            type_bridge_orm::AnswerCancellation::default()
        } else {
            // SAFETY: caller retains the immutable cancellation handle through decode.
            unsafe { &*cancellation }.answer_cancellation()
        };
        if let Err(error) = claim.deadline.check(&answer) {
            if let Err(consumed) = take_remote_claim(claim) {
                return return_execution_error(consumed, out_diagnostics);
            }
            return return_execution_error(error, out_diagnostics);
        }
        if response.length > claim.response_snapshot_limit {
            if let Err(error) = take_remote_claim(claim) {
                return return_execution_error(error, out_diagnostics);
            }
            return return_execution_error(remote_response_limit(), out_diagnostics);
        }
        let reservation = match ReservedBox::try_new(AllocationSite::QueryResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: the canonical response range was preflighted and remains immutable.
        let response =
            match unsafe { copy_query_bytes(response, AllocationSite::QueryRemoteResponseBytes) } {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
            };
        if let Err(error) = claim.deadline.check(&answer) {
            if let Err(consumed) = take_remote_claim(claim) {
                return return_execution_error(consumed, out_diagnostics);
            }
            return return_execution_error(error, out_diagnostics);
        }
        let claimed = match take_remote_claim(claim) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let (request, result, registry) = match claimed.decode_with_cancellation(&response, &answer)
        {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower_remote(error), out_diagnostics),
        };
        let result = match materialize_result(
            &claim.terminal,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            MaterializationBudget {
                limits: claim.state.limits.projected(),
                cancellation: &answer,
                deadline: claim.deadline,
            },
        ) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let result = reservation.initialize(result);
        // SAFETY: the output slot was preflighted and initialized.
        unsafe { out_result.write_unaligned(Box::into_raw(result)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one remote reply claim handle, decoded or still live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_remote_claim_close(
    claim: *mut *mut TypeBridgeQueryRemoteClaim,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(claim) }
}

fn rows(value: &ProjectedQueryValue) -> Option<&[type_bridge_orm::ProjectedQueryRow]> {
    match value {
        ProjectedQueryValue::Rows { rows } => Some(rows),
        ProjectedQueryValue::Page { entries, .. } => Some(entries),
        _ => None,
    }
}

fn reduction_rows(value: &ProjectedQueryValue) -> Option<&[ProjectedReductionRow]> {
    match value {
        ProjectedQueryValue::Reduction { rows, .. }
        | ProjectedQueryValue::FieldReduction { rows, .. }
        | ProjectedQueryValue::FieldTupleReduction { rows, .. } => Some(rows),
        _ => None,
    }
}

fn reduction_kind_matches(result: &TypeBridgeQueryResult, expected_kind: u32) -> bool {
    matches!(
        expected_kind,
        RESULT_REDUCTION | RESULT_FIELD_REDUCTION | RESULT_FIELD_TUPLE_REDUCTION
    ) && query_result_kind(result.value.value()) == expected_kind
}

unsafe fn result_scalar_preflight<T: Copy>(
    result: *const TypeBridgeQueryResult,
    out_value: *mut T,
    default: T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = direct_output_preflight(&[
        (out_value.cast(), size_of::<T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])?;
    // SAFETY: complete outer result object is checked before dereference.
    unsafe { preflight_handle(&preflight, result) }?;
    if !result.is_null() {
        // SAFETY: caller retains the immutable result handle.
        unsafe { &*result }.check_borrowed_ranges(&preflight)?;
    }
    // SAFETY: full alias preflight precedes output initialization.
    unsafe { initialize_scalar_output(out_value, default, out_diagnostics) }?;
    Ok(preflight)
}

/// Return one materialized result's closed terminal-result kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_kind(
    result: *const TypeBridgeQueryResult,
    out_kind: *mut u32,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[(out_kind.cast(), size_of::<u32>())]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer result object is checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, result) } {
        return status;
    }
    if !result.is_null()
        // SAFETY: caller retains the immutable result handle.
        && let Err(status) = unsafe { &*result }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    guarded(|| {
        if out_kind.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplies one writable output slot.
        unsafe { out_kind.write_unaligned(0) };
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result handle.
        let result = unsafe { &*result };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_kind.write_unaligned(query_result_kind(result.value.value())) };
        TypeBridgeStatus::Ok
    })
}

/// Return the number of rows after exact result-kind fencing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_row_count(
    result: *const TypeBridgeQueryResult,
    expected_kind: u32,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs only after full alias checks.
    let _preflight = match unsafe { result_scalar_preflight(result, out_count, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() || !matches!(expected_kind, RESULT_ROWS | RESULT_PAGE) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result handle.
        let result = unsafe { &*result };
        if query_result_kind(result.value.value()) != expected_kind {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let values = rows(result.value.value()).expect("kind checked as row result");
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_count.write_unaligned(values.len()) };
        TypeBridgeStatus::Ok
    })
}

unsafe fn checked_row_slot(
    result: &TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    slot_index: usize,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    expected_kind: u32,
) -> Result<&type_bridge_orm::ProjectedQuerySlot, SdkExecutionDiagnostic> {
    if !matches!(expected_result_kind, RESULT_ROWS | RESULT_PAGE)
        || query_result_kind(result.value.value()) != expected_result_kind
        || !matches!(expected_kind, SELECTION_ONE | SELECTION_COLLECT)
    {
        return Err(result_mismatch());
    }
    // SAFETY: generated token storage was preflighted and remains readable.
    let expected = unsafe { resolve_expected_model(&result.state, expected_model) }?;
    let Some(mode) = match_mode(expected_mode) else {
        return Err(layout_invalid());
    };
    let row = rows(result.value.value())
        .and_then(|rows| rows.get(row_index))
        .ok_or_else(result_index_invalid)?;
    let slot = row
        .slots()
        .get(slot_index)
        .ok_or_else(result_index_invalid)?;
    let actual_kind = match slot.value() {
        ProjectedQuerySlotValue::One(_) => SELECTION_ONE,
        ProjectedQuerySlotValue::Many(_) => SELECTION_COLLECT,
    };
    if slot.declared_type() != &expected
        || slot.match_mode() != mode
        || actual_kind != expected_kind
    {
        return Err(result_mismatch());
    }
    Ok(slot)
}

/// Return one exact generated row-slot multiplicity.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn type_bridge_query_result_row_slot_count(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    slot_index: usize,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    expected_kind: u32,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[
        (out_count.cast(), size_of::<usize>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for check in [unsafe { preflight_handle(&preflight, result) }, unsafe {
        preflight_handle(&preflight, expected_model)
    }] {
        if let Err(status) = check {
            return status;
        }
    }
    if !result.is_null()
        // SAFETY: caller retains the immutable result handle.
        && let Err(status) = unsafe { &*result }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_scalar_output(out_count, 0, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the result and generated token.
        let result = unsafe { &*result };
        // SAFETY: generated token storage was completely preflighted.
        let slot = match unsafe {
            checked_row_slot(
                result,
                expected_result_kind,
                row_index,
                slot_index,
                expected_model,
                expected_mode,
                expected_kind,
            )
        } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let count = match slot.value() {
            ProjectedQuerySlotValue::One(_) => 1,
            ProjectedQuerySlotValue::Many(values) => values.len(),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_count.write_unaligned(count) };
        TypeBridgeStatus::Ok
    })
}

/// Clone one exact/subtype-fenced projected thing from a selected row slot.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn type_bridge_query_result_row_slot_thing_at(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    slot_index: usize,
    thing_index: usize,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    expected_kind: u32,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for check in [unsafe { preflight_handle(&preflight, result) }, unsafe {
        preflight_handle(&preflight, expected_model)
    }] {
        if let Err(status) = check {
            return status;
        }
    }
    if !result.is_null()
        // SAFETY: caller retains the immutable result graph.
        && let Err(status) = unsafe { &*result }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full result-graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_thing, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result and generated token.
        let result = unsafe { &*result };
        // SAFETY: generated token storage was completely preflighted.
        let slot = match unsafe {
            checked_row_slot(
                result,
                expected_result_kind,
                row_index,
                slot_index,
                expected_model,
                expected_mode,
                expected_kind,
            )
        } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let thing = match slot.value() {
            ProjectedQuerySlotValue::One(value) if thing_index == 0 => Arc::clone(value),
            ProjectedQuerySlotValue::One(_) => {
                return return_execution_error(result_index_invalid(), out_diagnostics);
            }
            ProjectedQuerySlotValue::Many(values) => match values.get(thing_index) {
                Some(value) => Arc::clone(value),
                None => return return_execution_error(result_index_invalid(), out_diagnostics),
            },
        };
        write_handle(
            AllocationSite::QueryThingHandle,
            TypeBridgeProjectedThing::from_arc(Arc::clone(&result.state.package), thing),
            out_thing,
            out_diagnostics,
        )
    })
}

/// Return page window and optional same-snapshot total metadata.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_page_metadata_v1(
    result: *const TypeBridgeQueryResult,
    out_metadata: *mut TypeBridgeQueryPageMetadataV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let default = TypeBridgeQueryPageMetadataV1 {
        struct_size: 0,
        version: 0,
        offset: 0,
        limit: 0,
        has_total: 0,
        reserved0: [0; 7],
        total: 0,
        reserved: [0; 4],
    };
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight =
        match unsafe { result_scalar_preflight(result, out_metadata, default, out_diagnostics) } {
            Ok(value) => value,
            Err(status) => return status,
        };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result handle.
        let result = unsafe { &*result };
        let ProjectedQueryValue::Page { window, total, .. } = result.value.value() else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        let struct_size = match u32::try_from(size_of::<TypeBridgeQueryPageMetadataV1>()) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::Panic,
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe {
            out_metadata.write_unaligned(TypeBridgeQueryPageMetadataV1 {
                struct_size,
                version: DESCRIPTOR_VERSION,
                offset: window.offset,
                limit: window.limit,
                has_total: u8::from(total.is_some()),
                reserved0: [0; 7],
                total: total.unwrap_or(0),
                reserved: [0; 4],
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Return a distinct-root count result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_count(
    result: *const TypeBridgeQueryResult,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_count, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        let ProjectedQueryValue::Count { value, .. } = result.value.value() else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_count.write_unaligned(*value) };
        TypeBridgeStatus::Ok
    })
}

/// Return a distinct-root existence result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_exists(
    result: *const TypeBridgeQueryResult,
    out_exists: *mut u8,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight =
        match unsafe { result_scalar_preflight(result, out_exists, 0, out_diagnostics) } {
            Ok(value) => value,
            Err(status) => return status,
        };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        let ProjectedQueryValue::Exists { value, .. } = result.value.value() else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_exists.write_unaligned(u8::from(*value)) };
        TypeBridgeStatus::Ok
    })
}

/// Return the number of rows in any typed reduction result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_row_count(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_count, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let Some(rows) = reduction_rows(result.value.value()) else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_count.write_unaligned(rows.len()) };
        TypeBridgeStatus::Ok
    })
}

fn reduction_row(
    result: &TypeBridgeQueryResult,
    row_index: usize,
) -> Result<&ProjectedReductionRow, SdkExecutionDiagnostic> {
    reduction_rows(result.value.value())
        .and_then(|values| values.get(row_index))
        .ok_or_else(result_index_invalid)
}

/// Return the group-key kind for one reduction row.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_group_kind(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    out_kind: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight =
        match unsafe { result_scalar_preflight(result, out_kind, GROUP_NONE, out_diagnostics) } {
            Ok(value) => value,
            Err(status) => return status,
        };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let row = match reduction_row(result, row_index) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let kind = match row.group() {
            None => GROUP_NONE,
            Some(ProjectedReductionGroup::Thing(_)) => GROUP_THING,
            Some(ProjectedReductionGroup::Field(_)) => GROUP_FIELD,
            Some(ProjectedReductionGroup::Fields(_)) => GROUP_FIELDS,
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_kind.write_unaligned(kind) };
        TypeBridgeStatus::Ok
    })
}

fn binding_group_contract(
    result: &TypeBridgeQueryResult,
) -> Option<(type_bridge_orm::BindingId, &type_bridge_orm::MatchBinding)> {
    let ProjectedQueryValue::Reduction {
        group: Some(group), ..
    } = result.value.value()
    else {
        return None;
    };
    let binding = result
        .value
        .request_proof()
        .request()
        .plan
        .bindings
        .iter()
        .find(|binding| binding.id == *group)?;
    Some((*group, binding))
}

/// Clone one generated-model-fenced binding reduction group thing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_group_thing(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_mode: u32,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for check in [unsafe { preflight_handle(&preflight, result) }, unsafe {
        preflight_handle(&preflight, expected_model)
    }] {
        if let Err(status) = check {
            return status;
        }
    }
    if !result.is_null()
        // SAFETY: caller retains the complete immutable result graph.
        && let Err(status) = unsafe { &*result }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full result-graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_thing, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the result and generated token.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        // SAFETY: generated model token was completely preflighted.
        let expected = match unsafe { resolve_expected_model(&result.state, expected_model) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let Some(mode) = match_mode(expected_mode) else {
            return return_execution_error(layout_invalid(), out_diagnostics);
        };
        let Some((_, binding)) = binding_group_contract(result) else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        let expected_descriptor = match result
            .state
            .registry
            .descriptor_id(type_descriptor_name(&expected))
        {
            Some(value) => value,
            None => return return_execution_error(result_mismatch(), out_diagnostics),
        };
        if binding.descriptor != expected_descriptor || binding.match_mode != mode {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let row = match reduction_row(result, row_index) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let Some(ProjectedReductionGroup::Thing(value)) = row.group() else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        write_handle(
            AllocationSite::QueryThingHandle,
            TypeBridgeProjectedThing::from_arc(
                Arc::clone(&result.state.package),
                Arc::clone(value),
            ),
            out_thing,
            out_diagnostics,
        )
    })
}

/// Return the scalar-field count in one reduction group key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_group_field_count(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_count, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let row = match reduction_row(result, row_index) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let count = match row.group() {
            Some(ProjectedReductionGroup::Field(_)) => 1,
            Some(ProjectedReductionGroup::Fields(values)) => values.len(),
            _ => return return_execution_error(result_mismatch(), out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_count.write_unaligned(count) };
        TypeBridgeStatus::Ok
    })
}

fn grouped_field(
    result: &TypeBridgeQueryResult,
    field_index: usize,
) -> Option<&type_bridge_orm::BoundFieldId> {
    match result.value.value() {
        ProjectedQueryValue::FieldReduction { group, .. } if field_index == 0 => Some(group),
        ProjectedQueryValue::FieldTupleReduction { groups, .. } => groups.get(field_index),
        _ => None,
    }
}

fn field_contract_matches(
    result: &TypeBridgeQueryResult,
    grouped: &type_bridge_orm::BoundFieldId,
    owner: &TypeId,
    field: &OwnsFactId,
) -> bool {
    let Some(owner_descriptor) = result
        .state
        .registry
        .descriptor_id(type_descriptor_name(owner))
    else {
        return false;
    };
    let Some(name) = field_target_name(&result.state, owner, field) else {
        return false;
    };
    grouped.field.owner == owner_descriptor && grouped.field.name == name
}

/// Clone one exact generated-field-fenced scalar group value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_group_field_at(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    field_index: usize,
    expected_field: *const TypeBridgeProjectedTokenV1,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_value, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer objects are checked before dereference.
    for check in [unsafe { preflight_handle(&preflight, result) }, unsafe {
        preflight_handle(&preflight, expected_field)
    }] {
        if let Err(status) = check {
            return status;
        }
    }
    if !result.is_null()
        // SAFETY: caller retains the complete immutable result graph.
        && let Err(status) = unsafe { &*result }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full result-graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result and generated token.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        // SAFETY: generated field token was completely preflighted.
        let (owner, field) =
            match unsafe { resolve_field_token(&result.state.package, expected_field) } {
                Ok(value) => value,
                Err(error) => return return_execution_error(error, out_diagnostics),
            };
        let Some(grouped) = grouped_field(result, field_index) else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        if !field_contract_matches(result, grouped, &owner, &field) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let row = match reduction_row(result, row_index) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let value = match row.group() {
            Some(ProjectedReductionGroup::Field(value)) if field_index == 0 => value,
            Some(ProjectedReductionGroup::Fields(values)) => match values.get(field_index) {
                Some(value) => value,
                None => return return_execution_error(result_index_invalid(), out_diagnostics),
            },
            _ => return return_execution_error(result_mismatch(), out_diagnostics),
        };
        write_handle(
            AllocationSite::QueryValueHandle,
            TypeBridgeProjectedValue::from_arc(
                Arc::clone(&result.state.package),
                Arc::clone(value),
            ),
            out_value,
            out_diagnostics,
        )
    })
}

/// Return kind and optionality metadata for one reducer output cell.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_value_metadata_v1(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    value_index: usize,
    expected_value_kind: u32,
    out_metadata: *mut TypeBridgeQueryReducedValueMetadataV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let default = TypeBridgeQueryReducedValueMetadataV1 {
        struct_size: 0,
        version: 0,
        kind: 0,
        present: 0,
        reserved0: [0; 3],
        reserved: [0; 4],
    };
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight =
        match unsafe { result_scalar_preflight(result, out_metadata, default, out_diagnostics) } {
            Ok(value) => value,
            Err(status) => return status,
        };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let row = match reduction_row(result, row_index) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let Some(value) = row.values().get(value_index) else {
            return return_execution_error(result_index_invalid(), out_diagnostics);
        };
        let (kind, present) = match value {
            ProjectedReducedValue::Count(_) => (REDUCED_COUNT, true),
            ProjectedReducedValue::Long(value) => (REDUCED_LONG, value.is_some()),
            ProjectedReducedValue::Double(value) => (REDUCED_DOUBLE, value.is_some()),
        };
        if kind != expected_value_kind {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let struct_size = match u32::try_from(size_of::<TypeBridgeQueryReducedValueMetadataV1>()) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::Panic,
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe {
            out_metadata.write_unaligned(TypeBridgeQueryReducedValueMetadataV1 {
                struct_size,
                version: DESCRIPTOR_VERSION,
                kind,
                present: u8::from(present),
                reserved0: [0; 3],
                reserved: [0; 4],
            })
        };
        TypeBridgeStatus::Ok
    })
}

fn reduced_value(
    result: &TypeBridgeQueryResult,
    row_index: usize,
    value_index: usize,
) -> Result<&ProjectedReducedValue, SdkExecutionDiagnostic> {
    reduction_row(result, row_index)?
        .values()
        .get(value_index)
        .ok_or_else(result_index_invalid)
}

/// Return one exact count reducer cell.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_value_count(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    value_index: usize,
    out_value: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_value, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let value = match reduced_value(result, row_index, value_index) {
            Ok(ProjectedReducedValue::Count(value)) => *value,
            Ok(_) => return return_execution_error(result_mismatch(), out_diagnostics),
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_value.write_unaligned(value) };
        TypeBridgeStatus::Ok
    })
}

/// Return one present long-domain reducer cell.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_value_long(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    value_index: usize,
    out_value: *mut i64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_value, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let value = match reduced_value(result, row_index, value_index) {
            Ok(ProjectedReducedValue::Long(Some(value))) => *value,
            Ok(_) => return return_execution_error(result_mismatch(), out_diagnostics),
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_value.write_unaligned(value) };
        TypeBridgeStatus::Ok
    })
}

/// Return exact finite IEEE-754 bits from one present double reducer cell.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_reduction_value_double_bits(
    result: *const TypeBridgeQueryResult,
    expected_result_kind: u32,
    row_index: usize,
    value_index: usize,
    out_bits: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared scalar-result preflight initializes outputs after full result traversal.
    let _preflight = match unsafe { result_scalar_preflight(result, out_bits, 0, out_diagnostics) }
    {
        Ok(value) => value,
        Err(status) => return status,
    };
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable result.
        let result = unsafe { &*result };
        if !reduction_kind_matches(result, expected_result_kind) {
            return return_execution_error(result_mismatch(), out_diagnostics);
        }
        let value = match reduced_value(result, row_index, value_index) {
            Ok(ProjectedReducedValue::Double(Some(value))) => value.bits(),
            Ok(_) => return return_execution_error(result_mismatch(), out_diagnostics),
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_bits.write_unaligned(value) };
        TypeBridgeStatus::Ok
    })
}

/// Close one materialized typed-query result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_query_result_close(
    result: *mut *mut TypeBridgeQueryResult,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(result) }
}

/// Return one projected thing's exact generated model-token ordinal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_model_ordinal(
    thing: *const TypeBridgeProjectedThing,
    out_ordinal: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[
        (out_ordinal.cast(), size_of::<u32>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer object is checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, thing) } {
        return status;
    }
    if !thing.is_null()
        // SAFETY: caller retains the complete immutable projected thing graph.
        && let Err(status) = unsafe { &*thing }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full thing-graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_scalar_output(out_ordinal, 0, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable projected thing.
        let thing = unsafe { &*thing };
        let identity = ProjectedTokenIdentity::Model(thing.value.type_id().clone());
        let Some(ordinal) = thing.package._projection.projected_token_ordinal(&identity) else {
            return return_execution_error(result_mismatch(), out_diagnostics);
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_ordinal.write_unaligned(ordinal) };
        TypeBridgeStatus::Ok
    })
}

/// Clone one projected thing handle by cloning its shared immutable Arc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_thing_clone(
    thing: *const TypeBridgeProjectedThing,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: complete outer object is checked before dereference.
    if let Err(status) = unsafe { preflight_handle(&preflight, thing) } {
        return status;
    }
    if !thing.is_null()
        // SAFETY: caller retains the complete immutable projected thing graph.
        && let Err(status) = unsafe { &*thing }.check_borrowed_ranges(&preflight)
    {
        return status;
    }
    // SAFETY: full thing-graph alias preflight precedes output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_thing, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if thing.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable projected thing.
        let thing = unsafe { &*thing };
        write_handle(
            AllocationSite::QueryThingHandle,
            TypeBridgeProjectedThing::from_arc(
                Arc::clone(&thing.package),
                Arc::clone(&thing.value),
            ),
            out_thing,
            out_diagnostics,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::env;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::mem::{offset_of, size_of};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use sha2::{Digest as _, Sha256};

    use type_bridge_contract::capability::{CapabilityId, CapabilitySet as ContractCapabilitySet};
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPath, DiagnosticPathSegment,
    };
    use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::BindingId as ContractBindingId;
    use type_bridge_contract::projection::{
        ProjectedTokenKind, TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
    };
    use type_bridge_contract::query_plan::{ModelQueryV2, query_plan_v2_capability_vocabulary};
    use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
    use type_bridge_contract::query_remote_v2::{
        HydrationGraphV2, RemoteOutcomeV2, RemoteQueryFailureV2, RemoteQueryRequestV2,
        RemoteQueryResponseV2, RemoteReducedValueV2, RemoteReductionRowV2, RemoteResultKindV2,
        query_remote_v2_required_capabilities,
    };
    use type_bridge_core_lib::ast::{
        TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
    };
    use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
    use type_bridge_orm::session::backend::{
        AnswerConsumer, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader, BoundedAnswerStats,
        BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };
    use type_bridge_orm::{CapabilitySet, Database, OrmError, TxType};

    use super::*;
    use crate::abi::{TypeBridgeByteView, type_bridge_schema_package_close};
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{model_token, package};
    use crate::execution_diagnostic::{
        TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailKind,
        TypeBridgeExecutionDiagnosticDetailViewV1, TypeBridgeExecutionDiagnosticPathKind,
        TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
        type_bridge_execution_diagnostics_close, type_bridge_execution_diagnostics_detail_get_v1,
        type_bridge_execution_diagnostics_detail_list_count,
        type_bridge_execution_diagnostics_detail_list_get,
        type_bridge_execution_diagnostics_detail_signed, type_bridge_execution_diagnostics_get_v1,
        type_bridge_execution_diagnostics_path_get_v1,
    };
    use crate::projected_model::{
        type_bridge_projected_thing_close, type_bridge_projected_thing_iid,
    };
    use crate::projected_value::{
        type_bridge_projected_value_close, type_bridge_projected_value_string_open,
        type_bridge_projected_value_text,
    };
    use crate::runtime::{
        type_bridge_cancellation_close, type_bridge_cancellation_open,
        type_bridge_cancellation_request, type_bridge_read_transaction_close,
        type_bridge_read_transaction_open,
    };

    enum Response {
        Items(Vec<AnswerItem>),
        Error,
        Panic,
        Pending,
    }

    #[derive(Default)]
    struct Events {
        opens: AtomicUsize,
        queries: AtomicUsize,
        hydrations: AtomicUsize,
        closes: AtomicUsize,
    }

    struct FakeBackend {
        events: Arc<Events>,
        queries: Arc<Mutex<VecDeque<Response>>>,
        hydrations: Arc<Mutex<VecDeque<Response>>>,
        close_failure: bool,
        open_pending: bool,
    }

    impl DriverBackend for FakeBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.events.opens.fetch_add(1, Ordering::AcqRel);
            if self.open_pending {
                return Box::pin(std::future::pending());
            }
            let transaction = FakeTransaction {
                events: Arc::clone(&self.events),
                queries: Arc::clone(&self.queries),
                hydrations: Arc::clone(&self.hydrations),
                close_failure: self.close_failure,
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct FakeTransaction {
        events: Arc<Events>,
        queries: Arc<Mutex<VecDeque<Response>>>,
        hydrations: Arc<Mutex<VecDeque<Response>>>,
        close_failure: bool,
    }

    fn feed(
        items: Vec<AnswerItem>,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, OrmError> {
        let mut reader = BoundedAnswerReader::new(limits);
        reader.check_before_read()?;
        for item in items {
            if reader.accept(item, consumer)?
                == type_bridge_orm::session::backend::AnswerControl::Stop
            {
                break;
            }
        }
        Ok(reader.stats())
    }

    impl TransactionOps for FakeTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("typed-query tests must not use raw TypeQL") })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.events.queries.fetch_add(1, Ordering::AcqRel);
            let statement_limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
            let response = self
                .queries
                .lock()
                .expect("query response queue lock")
                .pop_front()
                .unwrap_or_else(|| Response::Items(Vec::new()));
            Box::pin(async move {
                match response {
                    Response::Items(items) => feed(
                        items.into_iter().take(statement_limit).collect(),
                        limits,
                        consumer,
                    ),
                    Response::Error => Err(OrmError::QueryExecution(
                        "provider-secret-query-failure".to_owned(),
                    )),
                    Response::Panic => panic!("injected typed-query provider panic"),
                    Response::Pending => std::future::pending().await,
                }
            })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.events.hydrations.fetch_add(1, Ordering::AcqRel);
            let response = self
                .hydrations
                .lock()
                .expect("hydration response queue lock")
                .pop_front()
                .unwrap_or_else(|| Response::Items(Vec::new()));
            Box::pin(async move {
                match response {
                    Response::Items(items) => feed(items, limits, consumer),
                    Response::Error => Err(OrmError::QueryExecution(
                        "provider-secret-hydration-failure".to_owned(),
                    )),
                    Response::Panic => panic!("injected typed-query hydration panic"),
                    Response::Pending => std::future::pending().await,
                }
            })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.events.queries.fetch_add(1, Ordering::AcqRel);
            let response = self
                .queries
                .lock()
                .expect("root response queue lock")
                .pop_front()
                .unwrap_or_else(|| Response::Items(Vec::new()));
            Box::pin(async move {
                match response {
                    Response::Items(items) => feed(items, limits, consumer),
                    Response::Error => Err(OrmError::QueryExecution(
                        "provider-secret-root-failure".to_owned(),
                    )),
                    Response::Panic => panic!("injected typed-query root panic"),
                    Response::Pending => std::future::pending().await,
                }
            })
        }

        fn rematch_page_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedPageRematch,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.events.hydrations.fetch_add(1, Ordering::AcqRel);
            let response = self
                .hydrations
                .lock()
                .expect("rematch response queue lock")
                .pop_front()
                .unwrap_or_else(|| Response::Items(Vec::new()));
            Box::pin(async move {
                match response {
                    Response::Items(items) => feed(items, limits, consumer),
                    Response::Error => Err(OrmError::QueryExecution(
                        "provider-secret-rematch-failure".to_owned(),
                    )),
                    Response::Panic => panic!("injected typed-query rematch panic"),
                    Response::Pending => std::future::pending().await,
                }
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("read query must not commit") })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.events.closes.fetch_add(1, Ordering::AcqRel);
            let close_failure = self.close_failure;
            Box::pin(async move {
                if close_failure {
                    Err(OrmError::Transaction(
                        "provider-secret-close-failure".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct FakeDatabase {
        value: Box<TypeBridgeDatabase>,
        events: Arc<Events>,
    }

    fn fake_database(
        package: &TypeBridgeSchemaPackage,
        queries: Vec<Response>,
        hydrations: Vec<Response>,
    ) -> FakeDatabase {
        fake_database_with_close_failure(package, queries, hydrations, false)
    }

    fn fake_database_with_close_failure(
        package: &TypeBridgeSchemaPackage,
        queries: Vec<Response>,
        hydrations: Vec<Response>,
        close_failure: bool,
    ) -> FakeDatabase {
        let events = Arc::new(Events::default());
        let database = Database::with_backend(
            Box::new(FakeBackend {
                events: Arc::clone(&events),
                queries: Arc::new(Mutex::new(queries.into())),
                hydrations: Arc::new(Mutex::new(hydrations.into())),
                close_failure,
                open_pending: false,
            }),
            "query-c-abi",
        );
        FakeDatabase {
            value: Box::new(TypeBridgeDatabase::from_test_database(
                Arc::clone(package.state()),
                database,
            )),
            events,
        }
    }

    fn fake_database_with_pending_open(package: &TypeBridgeSchemaPackage) -> FakeDatabase {
        let events = Arc::new(Events::default());
        let database = Database::with_backend(
            Box::new(FakeBackend {
                events: Arc::clone(&events),
                queries: Arc::new(Mutex::new(VecDeque::new())),
                hydrations: Arc::new(Mutex::new(VecDeque::new())),
                close_failure: false,
                open_pending: true,
            }),
            "query-c-abi",
        );
        FakeDatabase {
            value: Box::new(TypeBridgeDatabase::from_test_database(
                Arc::clone(package.state()),
                database,
            )),
            events,
        }
    }

    fn solution(iid: &str) -> AnswerItem {
        solution_bindings(&[(0, iid)])
    }

    fn solution_bindings(bindings: &[(u16, &str)]) -> AnswerItem {
        AnswerItem::Row(serde_json::json!({
            "bindings": bindings
                .iter()
                .map(|(binding, iid)| serde_json::json!({
                    "binding": binding,
                    "concept_id": iid,
                }))
                .collect::<Vec<_>>(),
            "satisfied_role_edges": [],
        }))
    }

    fn projected_field_token(
        package: &TypeBridgeSchemaPackage,
        owner: &TypeId,
        attribute: &str,
    ) -> TypeBridgeProjectedTokenV1 {
        let field = OwnsFactId::new(
            owner.clone(),
            AttributeId::new(attribute).expect("test attribute identity"),
        )
        .expect("test owns identity");
        let identity = ProjectedTokenIdentity::Field {
            owner: owner.clone(),
            field,
        };
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: ProjectedTokenKind::Field.as_u32(),
            ordinal: package
                .state
                ._projection
                .projected_token_ordinal(&identity)
                .expect("test field is projected"),
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

    fn projected_role_token(
        package: &TypeBridgeSchemaPackage,
        owner: &TypeId,
        role: &str,
    ) -> TypeBridgeProjectedTokenV1 {
        let identity = ProjectedTokenIdentity::Role {
            owner: owner.clone(),
            role: RoleId::new(owner.label().as_str(), role).expect("test role identity"),
        };
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: ProjectedTokenKind::Role.as_u32(),
            ordinal: package
                .state
                ._projection
                .projected_token_ordinal(&identity)
                .expect("test role is projected"),
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

    fn projected_function_token(
        package: &TypeBridgeSchemaPackage,
        function: &str,
    ) -> TypeBridgeProjectedTokenV1 {
        let identity = ProjectedTokenIdentity::Function(
            FunctionId::new(function).expect("test function identity"),
        );
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: ProjectedTokenKind::Function.as_u32(),
            ordinal: package
                .state
                ._projection
                .projected_token_ordinal(&identity)
                .expect("test function is projected"),
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

    fn hydrated_thing_value(
        package: &TypeBridgeSchemaPackage,
        binding: u16,
        type_name: &str,
        iid: &str,
        attributes: &[(&str, &str)],
    ) -> serde_json::Value {
        let registry = package
            .state
            .installed_projection
            .match_registry()
            .expect("test registry");
        let descriptor = registry.descriptor_id(type_name).expect("test descriptor");
        let attributes = attributes
            .iter()
            .map(|(field, value)| {
                let field = registry
                    .field_id(&descriptor, field)
                    .expect("test field")
                    .name;
                serde_json::json!({
                    "field": field,
                    "value_type": "string",
                    "values": [value],
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "binding": binding,
            "concept_id": iid,
            "concrete_type": type_name,
            "kind": "entity",
            "attributes": attributes,
            "roles": [],
        })
    }

    fn hydration_value(
        package: &TypeBridgeSchemaPackage,
        iid: &str,
        identifier: &str,
    ) -> serde_json::Value {
        hydrated_thing_value(package, 0, "person", iid, &[("identifier", identifier)])
    }

    fn hydration(package: &TypeBridgeSchemaPackage, iid: &str, identifier: &str) -> AnswerItem {
        AnswerItem::Document(hydration_value(package, iid, identifier))
    }

    fn rematch(package: &TypeBridgeSchemaPackage, iid: &str, identifier: &str) -> AnswerItem {
        rematch_bindings(vec![hydration_value(package, iid, identifier)])
    }

    fn rematch_bindings(bindings: Vec<serde_json::Value>) -> AnswerItem {
        AnswerItem::Document(serde_json::json!({
            "bindings": bindings,
            "satisfied_role_edges": [],
        }))
    }

    unsafe fn close_diagnostics(value: &mut *mut TypeBridgeExecutionDiagnostics) {
        assert_eq!(
            // SAFETY: the slot uniquely owns the diagnostic handle.
            unsafe { type_bridge_execution_diagnostics_close(value) },
            TypeBridgeStatus::Ok,
        );
    }

    unsafe fn diagnostic_code(value: *const TypeBridgeExecutionDiagnostics) -> String {
        let mut view = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            // SAFETY: the retained handle and writable output are live.
            unsafe { type_bridge_execution_diagnostics_get_v1(value, 0, view.as_mut_ptr()) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: success initialized the complete view and its bytes borrow the handle.
        let view = unsafe { view.assume_init() };
        // SAFETY: the returned code view remains valid while the handle is live.
        let bytes = unsafe { std::slice::from_raw_parts(view.code.data, view.code.length) };
        String::from_utf8(bytes.to_vec()).expect("diagnostic code is UTF-8")
    }

    unsafe fn diagnostic_category(
        value: *const TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeExecutionDiagnosticCategory {
        let mut view = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            // SAFETY: the retained handle and writable output are live.
            unsafe { type_bridge_execution_diagnostics_get_v1(value, 0, view.as_mut_ptr()) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: success initialized the complete view.
        unsafe { view.assume_init() }.category
    }

    fn append_view(output: &mut Vec<u8>, value: TypeBridgeByteView) {
        if value.length != 0 {
            // SAFETY: callers pass views borrowed from a retained diagnostic.
            output
                .extend_from_slice(unsafe { std::slice::from_raw_parts(value.data, value.length) });
        }
    }

    unsafe fn view_string(value: TypeBridgeByteView) -> String {
        // SAFETY: callers pass a byte view borrowed from a retained handle.
        let bytes = unsafe { std::slice::from_raw_parts(value.data, value.length) };
        String::from_utf8(bytes.to_vec()).expect("diagnostic view is UTF-8")
    }

    unsafe fn diagnostic_visible_text(value: *const TypeBridgeExecutionDiagnostics) -> Vec<u8> {
        let mut output = Vec::new();
        let mut diagnostic = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_get_v1(value, 0, diagnostic.as_mut_ptr()) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: success initialized the complete borrowed view.
        let diagnostic = unsafe { diagnostic.assume_init() };
        append_view(&mut output, diagnostic.code);
        append_view(&mut output, diagnostic.message);
        for index in 0..diagnostic.path_count {
            let mut path =
                std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticPathViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_path_get_v1(
                        value,
                        0,
                        index,
                        path.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok,
            );
            // SAFETY: success initialized the complete borrowed view.
            let path = unsafe { path.assume_init() };
            for text in [path.primary, path.secondary, path.tertiary] {
                append_view(&mut output, text);
            }
        }
        for index in 0..diagnostic.detail_count {
            let mut detail =
                std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticDetailViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_detail_get_v1(
                        value,
                        0,
                        index,
                        detail.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok,
            );
            // SAFETY: success initialized the complete borrowed view.
            let detail = unsafe { detail.assume_init() };
            for text in [
                detail.key,
                detail.primary,
                detail.secondary,
                detail.tertiary,
                detail.quaternary,
            ] {
                append_view(&mut output, text);
            }
            if detail.kind == TypeBridgeExecutionDiagnosticDetailKind::TextList {
                let mut count = 0;
                assert_eq!(
                    unsafe {
                        type_bridge_execution_diagnostics_detail_list_count(
                            value, 0, index, &mut count,
                        )
                    },
                    TypeBridgeStatus::Ok,
                );
                for list_index in 0..count {
                    let mut item = TypeBridgeByteView {
                        data: ptr::null(),
                        length: 0,
                    };
                    assert_eq!(
                        unsafe {
                            type_bridge_execution_diagnostics_detail_list_get(
                                value, 0, index, list_index, &mut item,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    append_view(&mut output, item);
                }
            }
        }
        output
    }

    fn request_cancellation_soon(
        cancellation: *const TypeBridgeCancellation,
    ) -> thread::JoinHandle<TypeBridgeStatus> {
        let address = cancellation.addr();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            // SAFETY: the execution call retains the cancellation handle until
            // this requester joins immediately after it returns.
            unsafe { type_bridge_cancellation_request(address as *const TypeBridgeCancellation) }
        })
    }

    fn raw_sha256(path: &Path) -> String {
        let bytes = fs::read(path)
            .unwrap_or_else(|error| panic!("proof source {} is readable: {error}", path.display()));
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn proof_source(root: &Path, relative: &str) -> serde_json::Value {
        serde_json::json!({
            "path": relative,
            "sha256": raw_sha256(&root.join(relative)),
        })
    }

    fn requested_workforce_v2_proof() -> Option<(PathBuf, String)> {
        let path = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT");
        let nonce = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE");
        assert_eq!(
            path.is_some(),
            nonce.is_some(),
            "proof fragment output and run nonce must be configured together",
        );
        let path = path?;
        let path = path
            .into_string()
            .expect("proof fragment output path must be UTF-8");
        assert!(path.len() <= 4096, "proof fragment output path is too long");
        let path = PathBuf::from(path);
        assert!(path.is_absolute(), "proof fragment output must be absolute");
        let parent = path.parent().expect("proof fragment output has a parent");
        let parent_metadata =
            fs::symlink_metadata(parent).expect("proof fragment parent must already exist");
        assert!(
            parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
            "proof fragment parent must be a non-symlink directory",
        );
        assert!(
            matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "proof fragment destination must not already exist",
        );
        let nonce = nonce
            .expect("proof run nonce exists")
            .into_string()
            .expect("proof run nonce must be UTF-8");
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "proof run nonce must be 64 lowercase hexadecimal characters",
        );
        Some((path, nonce))
    }

    fn publish_workforce_v2_proof(
        destination: &Path,
        run_nonce: &str,
        results: Vec<serde_json::Value>,
    ) {
        let core = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("C crate has a workspace root");
        let repository = core.parent().expect("core workspace has a repository root");
        let proof_schema =
            "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-schema-v1.json";
        let allowlist =
            "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-allowlist-v1.json";
        let journey = "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json";
        let sources = [
            "type-bridge-core/crates/c/include/typebridge/type_bridge.h",
            "type-bridge-core/crates/c/src/lib.rs",
            "type-bridge-core/crates/c/src/query.rs",
        ];
        let fragment = serde_json::json!({
            "format": "typebridge.workforce-v2-proof-fragment/v1",
            "binding": "c",
            "semantic_profile": "typedb-3.12.1/v1",
            "run_nonce": run_nonce,
            "contract": {
                "proof_schema": proof_source(repository, proof_schema),
                "allowlist": proof_source(repository, allowlist),
                "journey": proof_source(repository, journey),
            },
            "producer": {
                "id": "type-bridge-c.query-native-proof",
                "sources": sources
                    .iter()
                    .map(|source| proof_source(repository, source))
                    .collect::<Vec<_>>(),
            },
            "results": results,
        });
        let mut bytes = type_bridge_contract::codec::to_canonical_json(&fragment)
            .expect("proof fragment canonicalizes");
        bytes.push(b'\n');
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .expect("proof fragment destination is created once");
        output
            .write_all(&bytes)
            .expect("proof fragment is written completely");
        output.sync_all().expect("proof fragment is durable");
    }

    struct RemoteFixture {
        contract: RemoteCapabilities,
        advertisement: Vec<u8>,
        signer: RemoteReplySigningKey,
    }

    fn remote_fixture(seed: u8) -> RemoteFixture {
        let signer = RemoteReplySigningKey::from_secret_bytes([seed; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let contract = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new(
                format!("c-query-remote-{seed:02x}"),
                format!("epoch-{seed:011}"),
            )
            .expect("test remote executor binding"),
            signer.public_key(),
        );
        let advertisement = contract.encode().expect("test advertisement");
        RemoteFixture {
            contract,
            advertisement,
            signer,
        }
    }

    fn remote_count_reply(request: &[u8], fixture: &RemoteFixture, value: u64) -> Vec<u8> {
        let request = RemoteQueryRequestV2::decode(request).expect("test request envelope");
        request
            .validate_advertisement(&fixture.contract)
            .expect("request binds exact advertisement");
        let plan = request.plan().expect("test request plan");
        assert_eq!(request.result_kind(), RemoteResultKindV2::DistinctCount);
        RemoteQueryResponseV2::new(
            request.nonce(),
            &plan,
            &request.fingerprint().expect("request fingerprint"),
            RemoteResultKindV2::DistinctCount,
            RemoteOutcomeV2::DistinctCount {
                root: ContractBindingId::new(0).expect("test root binding"),
                value,
            },
        )
        .expect("request-bound count reply")
        .encode_signed(
            &fixture
                .contract
                .fingerprint()
                .expect("advertisement fingerprint"),
            &fixture.signer,
        )
        .expect("signed count reply")
    }

    #[derive(Clone, Copy, Debug)]
    enum HostileRemoteReply {
        MalformedOuter,
        BadSignature,
        WrongNonce,
        WrongRequest,
        ForeignSchemaOrProfilePlan,
        ForeignCapabilityAdvertisement,
        UnknownPayloadFormat,
    }

    fn hostile_remote_count_reply(
        request: &[u8],
        fixture: &RemoteFixture,
        kind: HostileRemoteReply,
    ) -> Vec<u8> {
        if matches!(kind, HostileRemoteReply::MalformedOuter) {
            return br#"{"secret-provider-reply":"must-not-escape""#.to_vec();
        }
        let request = RemoteQueryRequestV2::decode(request).expect("test request envelope");
        let plan = request.plan().expect("test request plan");
        let response = RemoteQueryResponseV2::new(
            request.nonce(),
            &plan,
            &request.fingerprint().expect("request fingerprint"),
            RemoteResultKindV2::DistinctCount,
            RemoteOutcomeV2::DistinctCount {
                root: ContractBindingId::new(0).expect("test root binding"),
                value: 19,
            },
        )
        .expect("request-bound count reply");
        let response = match kind {
            HostileRemoteReply::WrongNonce => {
                let mut value = serde_json::to_value(response).expect("response JSON");
                value["nonce"] = serde_json::json!("foreign-nonce-0000000000000000");
                serde_json::from_value(value).expect("syntactically valid foreign nonce response")
            }
            HostileRemoteReply::WrongRequest => {
                let mut value = serde_json::to_value(response).expect("response JSON");
                value["request"] = serde_json::json!("11".repeat(32));
                serde_json::from_value(value).expect("syntactically valid foreign request response")
            }
            HostileRemoteReply::ForeignSchemaOrProfilePlan => {
                let mut value = serde_json::to_value(response).expect("response JSON");
                value["plan"] = serde_json::json!("22".repeat(32));
                serde_json::from_value(value).expect("syntactically valid foreign plan response")
            }
            HostileRemoteReply::UnknownPayloadFormat => {
                let mut value = serde_json::to_value(response).expect("response JSON");
                value["format"] = serde_json::json!("typebridge.query-remote-response/v999");
                serde_json::from_value(value).expect("syntactically valid unknown response")
            }
            HostileRemoteReply::MalformedOuter
            | HostileRemoteReply::BadSignature
            | HostileRemoteReply::ForeignCapabilityAdvertisement => response,
        };
        let advertisement = if matches!(kind, HostileRemoteReply::ForeignCapabilityAdvertisement) {
            let mut capabilities = fixture.contract.capabilities().clone();
            capabilities.insert(
                CapabilityId::new("c.remote.hostile.extra-capability")
                    .expect("test extra capability"),
            );
            RemoteCapabilities::new(
                capabilities,
                fixture.contract.executor().clone(),
                fixture.signer.public_key(),
            )
            .fingerprint()
            .expect("foreign advertisement fingerprint")
        } else {
            fixture
                .contract
                .fingerprint()
                .expect("advertisement fingerprint")
        };
        let mut bytes = response
            .encode_signed(&advertisement, &fixture.signer)
            .expect("signed hostile response");
        if matches!(kind, HostileRemoteReply::BadSignature) {
            let marker = b"\"signature\":\"";
            let index = bytes
                .windows(marker.len())
                .position(|window| window == marker)
                .expect("signature member")
                + marker.len();
            bytes[index] = if bytes[index] == b'0' { b'1' } else { b'0' };
        }
        bytes
    }

    fn remote_terminal_reply(request: &[u8], fixture: &RemoteFixture) -> Vec<u8> {
        let request = RemoteQueryRequestV2::decode(request).expect("test request envelope");
        request
            .validate_advertisement(&fixture.contract)
            .expect("request binds exact advertisement");
        let plan = request.plan().expect("test request plan");
        let outcome = match request.result_kind() {
            RemoteResultKindV2::HydratedRows => RemoteOutcomeV2::HydratedRows {
                graph: HydrationGraphV2::new(vec![]).expect("empty hydration graph"),
                rows: vec![],
            },
            RemoteResultKindV2::HydratedPage => {
                let Some(ModelQueryV2::Page {
                    include_total,
                    root,
                    window,
                    ..
                }) = plan
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.model_query())
                else {
                    panic!("page request lacks its model contract")
                };
                RemoteOutcomeV2::HydratedPage {
                    entries: vec![],
                    graph: HydrationGraphV2::new(vec![]).expect("empty hydration graph"),
                    limit: window.limit(),
                    offset: window.offset(),
                    root: *root,
                    total: include_total.then_some(0),
                }
            }
            RemoteResultKindV2::DistinctExists => {
                let Some(ModelQueryV2::DistinctExists { root, .. }) = plan
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.model_query())
                else {
                    panic!("exists request lacks its model contract")
                };
                RemoteOutcomeV2::DistinctExists {
                    root: *root,
                    value: true,
                }
            }
            RemoteResultKindV2::ModelReduction => {
                let Some(ModelQueryV2::Reduction {
                    root,
                    group,
                    reducers,
                    ..
                }) = plan
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.model_query())
                else {
                    panic!("reduction request lacks its model contract")
                };
                RemoteOutcomeV2::ModelReduction {
                    graph: HydrationGraphV2::new(vec![]).expect("empty hydration graph"),
                    root: *root,
                    group: group.clone(),
                    reducers: reducers.clone(),
                    rows: vec![RemoteReductionRowV2::new(
                        None,
                        vec![RemoteReducedValueV2::Count { value: 13 }],
                    )],
                }
            }
            other => panic!("unexpected test remote result kind {other:?}"),
        };
        RemoteQueryResponseV2::new(
            request.nonce(),
            &plan,
            &request.fingerprint().expect("request fingerprint"),
            request.result_kind(),
            outcome,
        )
        .expect("request-bound terminal reply")
        .encode_signed(
            &fixture
                .contract
                .fingerprint()
                .expect("advertisement fingerprint"),
            &fixture.signer,
        )
        .expect("signed terminal reply")
    }

    unsafe fn open_remote_context(
        package: *const TypeBridgeSchemaPackage,
        advertisement: &[u8],
    ) -> *mut TypeBridgeQueryRemoteContext {
        // SAFETY: forwarded inputs obey this helper's contract.
        unsafe { open_remote_context_with_limits(package, advertisement, ptr::null()) }
    }

    unsafe fn open_remote_context_with_limits(
        package: *const TypeBridgeSchemaPackage,
        advertisement: &[u8],
        limits: *const TypeBridgeQueryExecutionLimitsV1,
    ) -> *mut TypeBridgeQueryRemoteContext {
        let mut context = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let view = TypeBridgeByteView {
            data: advertisement.as_ptr(),
            length: advertisement.len(),
        };
        assert_eq!(
            // SAFETY: caller retains all immutable inputs and writable independent outputs.
            unsafe {
                type_bridge_query_remote_context_open_v1(
                    package,
                    view,
                    limits,
                    &mut context,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        context
    }

    unsafe fn prepare_remote(
        context: *const TypeBridgeQueryRemoteContext,
        terminal: *const TypeBridgeQueryTerminal,
        kind: u32,
    ) -> *mut TypeBridgeQueryRemotePending {
        let mut pending = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: caller retains all immutable inputs and writable independent outputs.
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    terminal,
                    kind,
                    ptr::null(),
                    &mut pending,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        pending
    }

    unsafe fn pending_request(pending: *const TypeBridgeQueryRemotePending) -> TypeBridgeByteView {
        let mut request = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            // SAFETY: caller retains the pending handle and writable output.
            unsafe { type_bridge_query_remote_pending_request_bytes(pending, &mut request) },
            TypeBridgeStatus::Ok,
        );
        request
    }

    unsafe fn claim_remote(
        pending: *const TypeBridgeQueryRemotePending,
    ) -> *mut TypeBridgeQueryRemoteClaim {
        let mut claim = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: caller retains all immutable inputs and writable independent outputs.
            unsafe {
                type_bridge_query_remote_pending_claim(
                    pending,
                    ptr::null(),
                    &mut claim,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        claim
    }

    struct BuiltQuery {
        model: TypeBridgeProjectedTokenV1,
        session: *mut TypeBridgeQuerySession,
        binding: *mut TypeBridgeQueryBinding,
        selection: *mut TypeBridgeQuerySelection,
        query: *mut TypeBridgeQuery,
        terminal: *mut TypeBridgeQueryTerminal,
    }

    impl BuiltQuery {
        unsafe fn open(
            package: &TypeBridgeSchemaPackage,
            terminal_kind: u32,
            cardinality: u32,
        ) -> Self {
            let model = model_token(
                package,
                TypeId::new(TypeKind::Entity, "person").expect("test type identity"),
            );
            let mut diagnostics = ptr::null_mut();
            let mut session = ptr::null_mut();
            assert_eq!(
                // SAFETY: package and writable independent outputs are live.
                unsafe { type_bridge_query_session_open(package, &mut session, &mut diagnostics) },
                TypeBridgeStatus::Ok,
            );
            assert!(diagnostics.is_null());
            let mut binding = ptr::null_mut();
            assert_eq!(
                // SAFETY: retained session/token and writable outputs are live.
                unsafe {
                    type_bridge_query_binding_open_v1(
                        session,
                        &model,
                        MATCH_EXACT,
                        &mut binding,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let selection_descriptor = TypeBridgeQuerySelectionDescriptorV1 {
                struct_size: size_of::<TypeBridgeQuerySelectionDescriptorV1>() as u32,
                version: DESCRIPTOR_VERSION,
                binding,
                expected_model: &model,
                expected_mode: MATCH_EXACT,
                kind: SELECTION_ONE,
                distinct: 0,
                reserved0: [0; 7],
                orders: ptr::null(),
                order_count: 0,
                reserved: [0; 4],
            };
            let mut selection = ptr::null_mut();
            assert_eq!(
                // SAFETY: descriptor graph and writable outputs are live.
                unsafe {
                    type_bridge_query_selection_open_v1(
                        &selection_descriptor,
                        &mut selection,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let slot = TypeBridgeQueryShapeSlotV1 {
                struct_size: size_of::<TypeBridgeQueryShapeSlotV1>() as u32,
                version: DESCRIPTOR_VERSION,
                selection,
                expected_model: &model,
                expected_mode: MATCH_EXACT,
                expected_kind: SELECTION_ONE,
                reserved0: 0,
                name: TypeBridgeByteView {
                    data: ptr::null(),
                    length: 0,
                },
                reserved: [0; 4],
            };
            let query_descriptor = TypeBridgeQueryDescriptorV1 {
                struct_size: size_of::<TypeBridgeQueryDescriptorV1>() as u32,
                version: DESCRIPTOR_VERSION,
                shape_kind: SHAPE_POSITIONAL,
                reserved0: 0,
                slots: &slot,
                slot_count: 1,
                reserved: [0; 4],
            };
            let mut query = ptr::null_mut();
            assert_eq!(
                // SAFETY: descriptor graph and writable outputs are live.
                unsafe {
                    type_bridge_query_open_v1(
                        session,
                        &query_descriptor,
                        &mut query,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut terminal_descriptor = terminal_descriptor(terminal_kind, cardinality);
            if matches!(
                terminal_kind,
                TERMINAL_PAGE | TERMINAL_COUNT | TERMINAL_EXISTS | TERMINAL_REDUCE
            ) {
                terminal_descriptor.root = binding;
                terminal_descriptor.expected_root_model = &model;
                terminal_descriptor.expected_root_mode = MATCH_EXACT;
            }
            let count_reducer = TypeBridgeQueryReducerV1 {
                struct_size: size_of::<TypeBridgeQueryReducerV1>() as u32,
                version: DESCRIPTOR_VERSION,
                kind: REDUCER_COUNT,
                reserved0: 0,
                input: ptr::null(),
                expected_field: ptr::null(),
                reserved: [0; 4],
            };
            if terminal_kind == TERMINAL_REDUCE {
                terminal_descriptor.reducers = &count_reducer;
                terminal_descriptor.reducer_count = 1;
            }
            let mut terminal = ptr::null_mut();
            assert_eq!(
                // SAFETY: descriptor graph and writable outputs are live.
                unsafe {
                    type_bridge_query_terminal_open_v1(
                        query,
                        &terminal_descriptor,
                        &mut terminal,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert!(diagnostics.is_null());
            Self {
                model,
                session,
                binding,
                selection,
                query,
                terminal,
            }
        }

        unsafe fn close_builders(&mut self) {
            assert_eq!(
                unsafe { type_bridge_query_close(&mut self.query) },
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                unsafe { type_bridge_query_selection_close(&mut self.selection) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_binding_close(&mut self.binding) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_session_close(&mut self.session) },
                TypeBridgeStatus::Ok,
            );
        }
    }

    impl Drop for BuiltQuery {
        fn drop(&mut self) {
            // SAFETY: every non-null slot is uniquely owned by this fixture.
            unsafe {
                let _ = type_bridge_query_terminal_close(&mut self.terminal);
                let _ = type_bridge_query_close(&mut self.query);
                let _ = type_bridge_query_selection_close(&mut self.selection);
                let _ = type_bridge_query_binding_close(&mut self.binding);
                let _ = type_bridge_query_session_close(&mut self.session);
            }
        }
    }

    fn observe_workforce_v2_direct_cancellation() -> serde_json::Value {
        let package = package("queryproofdirectcancel");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let database = fake_database(&package, Vec::new(), Vec::new());
        let mut cancellation = ptr::null_mut();
        let mut result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let status = unsafe {
            type_bridge_database_query_execute_v1(
                &*database.value,
                built.terminal,
                TERMINAL_FIRST,
                ptr::null(),
                cancellation,
                &mut result,
                &mut diagnostics,
            )
        };
        assert_eq!(status, TypeBridgeStatus::Cancelled);
        let pre_dispatch_category = unsafe { diagnostic_category(diagnostics) };
        let pre_dispatch_code = unsafe { diagnostic_code(diagnostics) };
        let pre_dispatch_partial_result = !result.is_null();
        let pre_dispatch_provider_calls = database.events.opens.load(Ordering::Acquire)
            + database.events.queries.load(Ordering::Acquire)
            + database.events.hydrations.load(Ordering::Acquire);
        assert_eq!(
            pre_dispatch_category,
            TypeBridgeExecutionDiagnosticCategory::Cancelled,
        );
        assert_eq!(pre_dispatch_code, "provider_cancelled");
        assert!(!pre_dispatch_partial_result);
        assert_eq!(pre_dispatch_provider_calls, 0);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );

        let statement_database = fake_database(&package, vec![Response::Pending], Vec::new());
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        let requester = request_cancellation_soon(cancellation);
        result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
        let status = unsafe {
            type_bridge_database_query_execute_v1(
                &*statement_database.value,
                built.terminal,
                TERMINAL_FIRST,
                ptr::null(),
                cancellation,
                &mut result,
                &mut diagnostics,
            )
        };
        let provider_await_woken =
            requester.join().expect("cancellation requester joins") == TypeBridgeStatus::Ok;
        assert_eq!(status, TypeBridgeStatus::Cancelled);
        let in_flight_category = unsafe { diagnostic_category(diagnostics) };
        let in_flight_code = unsafe { diagnostic_code(diagnostics) };
        let in_flight_partial_result = !result.is_null();
        assert_eq!(
            in_flight_category,
            TypeBridgeExecutionDiagnosticCategory::Cancelled,
        );
        assert_eq!(in_flight_code, "provider_cancelled");
        assert!(!in_flight_partial_result);
        assert!(provider_await_woken);
        assert_eq!(statement_database.events.opens.load(Ordering::Acquire), 1);
        assert_eq!(statement_database.events.queries.load(Ordering::Acquire), 1);
        assert_eq!(statement_database.events.closes.load(Ordering::Acquire), 1);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );

        serde_json::json!({
            "in_flight": {
                "category": "cancelled",
                "code": in_flight_code,
                "partial_result": in_flight_partial_result,
                "provider_await_woken": provider_await_woken,
            },
            "pre_dispatch": {
                "category": "cancelled",
                "code": pre_dispatch_code,
                "partial_result": pre_dispatch_partial_result,
                "provider_calls": pre_dispatch_provider_calls,
            },
        })
    }

    fn observe_workforce_v2_remote_cancellation() -> serde_json::Value {
        let package = package("queryproofremotecancel");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x72);
        // SAFETY: package and advertisement remain immutable.
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let mut cancellation = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let mut pending = std::ptr::NonNull::<TypeBridgeQueryRemotePending>::dangling().as_ptr();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let before_exchange_status = unsafe {
            type_bridge_query_remote_prepare_v1(
                context,
                built.terminal,
                TERMINAL_COUNT,
                cancellation,
                &mut pending,
                &mut diagnostics,
            )
        };
        assert_eq!(before_exchange_status, TypeBridgeStatus::Cancelled);
        let before_exchange_code = unsafe { diagnostic_code(diagnostics) };
        let before_exchange_partial_result = !pending.is_null();
        assert_eq!(before_exchange_code, "provider_cancelled");
        assert!(!before_exchange_partial_result);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );

        // A caller can abandon a prepared request without claiming a reply or
        // invoking any native transport operation.
        pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let caller_transport_abort_supported =
            unsafe { type_bridge_query_remote_pending_close(&mut pending) == TypeBridgeStatus::Ok };
        assert!(caller_transport_abort_supported);
        assert!(pending.is_null());

        pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let request_view = unsafe { pending_request(pending) };
        let request =
            unsafe { std::slice::from_raw_parts(request_view.data, request_view.length).to_vec() };
        let mut claim = unsafe { claim_remote(pending) };
        let response = remote_count_reply(&request, &remote, 17);
        let exchange_count = usize::from(!response.is_empty());
        let response_view = TypeBridgeByteView {
            data: response.as_ptr(),
            length: response.len(),
        };
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let mut result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
        let during_decode_status = unsafe {
            type_bridge_query_remote_claim_decode_v1(
                claim,
                TERMINAL_COUNT,
                cancellation,
                response_view,
                &mut result,
                &mut diagnostics,
            )
        };
        assert_eq!(during_decode_status, TypeBridgeStatus::Cancelled);
        let during_decode_category = unsafe { diagnostic_category(diagnostics) };
        let during_decode_code = unsafe { diagnostic_code(diagnostics) };
        let during_decode_partial_result = !result.is_null();
        assert_eq!(
            during_decode_category,
            TypeBridgeExecutionDiagnosticCategory::Cancelled,
        );
        assert_eq!(during_decode_code, "provider_cancelled");
        assert!(!during_decode_partial_result);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_remote_claim_consumed",
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );

        serde_json::json!({
            "before_exchange": {
                "category": "cancelled",
                "code": before_exchange_code,
                "exchange_count": 0,
                "partial_result": before_exchange_partial_result,
            },
            "during_decode": {
                "category": "cancelled",
                "code": during_decode_code,
                "exchange_count": exchange_count,
                "partial_result": during_decode_partial_result,
            },
            "caller_transport_abort_supported": caller_transport_abort_supported,
            "server_exchange_cancelled_after_send": false,
        })
    }

    fn observe_workforce_v2_remote_structured_diagnostic() -> serde_json::Value {
        let package = package("queryproofremotediagnostic");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x53);
        // SAFETY: package and advertisement remain immutable.
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let mut pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let request_view = unsafe { pending_request(pending) };
        let request =
            unsafe { std::slice::from_raw_parts(request_view.data, request_view.length).to_vec() };
        let mut claim = unsafe { claim_remote(pending) };
        let envelope = RemoteQueryRequestV2::decode(&request).expect("test request envelope");
        let failure = Diagnostic::new(
            DiagnosticCategory::Integrity,
            DiagnosticCode::new("remote_application_failure").expect("test code"),
            "provider-secret remote query literal and endpoint",
        )
        .with_path(DiagnosticPath::from_segments([
            DiagnosticPathSegment::Field("plan".into()),
            DiagnosticPathSegment::Index(2),
            DiagnosticPathSegment::Identifier("person".into()),
        ]))
        .with_detail("attempt", -7_i64)
        .with_detail("expected", vec!["person".to_owned(), "employee".to_owned()])
        .with_detail("retryable", false)
        .with_detail("subject", "person")
        .with_detail("provider_secret", "must-not-cross");
        let response = RemoteQueryFailureV2::bound(
            envelope.nonce(),
            &envelope.fingerprint().expect("request fingerprint"),
            &failure,
        )
        .and_then(|failure| {
            failure.encode_signed(
                &remote
                    .contract
                    .fingerprint()
                    .expect("advertisement fingerprint"),
                &remote.signer,
            )
        })
        .expect("signed failure reply");
        let response_view = TypeBridgeByteView {
            data: response.as_ptr(),
            length: response.len(),
        };
        let mut result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(result.is_null());
        let mut diagnostic = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_get_v1(diagnostics, 0, diagnostic.as_mut_ptr())
            },
            TypeBridgeStatus::Ok,
        );
        let diagnostic = unsafe { diagnostic.assume_init() };
        assert_eq!(
            diagnostic.category,
            TypeBridgeExecutionDiagnosticCategory::Integrity,
        );
        let code = unsafe { view_string(diagnostic.code) };
        let message = unsafe { view_string(diagnostic.message) };
        assert_eq!(code, "remote_application_failure");
        let mut path_values = Vec::new();
        for index in 0..diagnostic.path_count {
            let mut path =
                std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticPathViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_path_get_v1(
                        diagnostics,
                        0,
                        index,
                        path.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let path = unsafe { path.assume_init() };
            path_values.push(match path.kind {
                TypeBridgeExecutionDiagnosticPathKind::ContractField => serde_json::json!({
                    "kind": "contract_field",
                    "value": unsafe { view_string(path.primary) },
                }),
                TypeBridgeExecutionDiagnosticPathKind::Index => serde_json::json!({
                    "kind": "index",
                    "value": path.index,
                }),
                TypeBridgeExecutionDiagnosticPathKind::ContractIdentity => serde_json::json!({
                    "kind": "contract_identity",
                    "value": unsafe { view_string(path.primary) },
                }),
                other => panic!("unexpected proof diagnostic path {other:?}"),
            });
        }
        let mut query_category = None;
        let mut details = serde_json::Map::new();
        for index in 0..diagnostic.detail_count {
            let mut detail =
                std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticDetailViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_detail_get_v1(
                        diagnostics,
                        0,
                        index,
                        detail.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let detail = unsafe { detail.assume_init() };
            let key = unsafe { view_string(detail.key) };
            match key.as_str() {
                "query_category" => {
                    query_category = Some(unsafe { view_string(detail.primary) });
                }
                "attempt" => {
                    let mut value = 0_i64;
                    assert_eq!(
                        unsafe {
                            type_bridge_execution_diagnostics_detail_signed(
                                diagnostics,
                                0,
                                index,
                                &mut value,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    details.insert(
                        key,
                        serde_json::json!({"kind": "signed", "value": value.to_string()}),
                    );
                }
                "expected" => {
                    let mut count = 0;
                    assert_eq!(
                        unsafe {
                            type_bridge_execution_diagnostics_detail_list_count(
                                diagnostics,
                                0,
                                index,
                                &mut count,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    let mut values = Vec::with_capacity(count);
                    for list_index in 0..count {
                        let mut value = TypeBridgeByteView {
                            data: ptr::null(),
                            length: 0,
                        };
                        assert_eq!(
                            unsafe {
                                type_bridge_execution_diagnostics_detail_list_get(
                                    diagnostics,
                                    0,
                                    index,
                                    list_index,
                                    &mut value,
                                )
                            },
                            TypeBridgeStatus::Ok,
                        );
                        values.push(unsafe { view_string(value) });
                    }
                    details.insert(
                        key,
                        serde_json::json!({"kind": "query_identity_list", "value": values}),
                    );
                }
                "retryable" => {
                    details.insert(
                        key,
                        serde_json::json!({
                            "kind": "boolean",
                            "value": detail.boolean_value != 0,
                        }),
                    );
                }
                "subject" => {
                    details.insert(
                        key,
                        serde_json::json!({
                            "kind": "query_identity",
                            "value": unsafe { view_string(detail.primary) },
                        }),
                    );
                }
                other => panic!("unexpected published remote proof detail {other}"),
            }
        }
        let visible = String::from_utf8(unsafe { diagnostic_visible_text(diagnostics) })
            .expect("diagnostic views are UTF-8");
        let redacted = !visible.contains("provider-secret") && !visible.contains("must-not-cross");
        assert!(redacted);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(result.is_null());
        let claim_consumed =
            unsafe { diagnostic_code(diagnostics) } == "c_query_remote_claim_consumed";
        assert!(claim_consumed);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );

        serde_json::json!({
            "category": "integrity",
            "query_category": query_category.expect("query category is retained"),
            "code": code,
            "message": message,
            "path": path_values,
            "details": details,
            "redacted": redacted,
            "claim_consumed": claim_consumed,
        })
    }

    fn terminal_descriptor(kind: u32, cardinality: u32) -> TypeBridgeQueryTerminalDescriptorV1 {
        let has_window = matches!(kind, TERMINAL_ROWS | TERMINAL_FIRST | TERMINAL_PAGE);
        TypeBridgeQueryTerminalDescriptorV1 {
            struct_size: size_of::<TypeBridgeQueryTerminalDescriptorV1>() as u32,
            version: DESCRIPTOR_VERSION,
            kind,
            cardinality,
            root: ptr::null(),
            expected_root_model: ptr::null(),
            expected_root_mode: 0,
            reserved1: 0,
            orders: ptr::null(),
            order_count: 0,
            offset: 0,
            limit: u64::from(has_window),
            include_total: 0,
            reserved0: [0; 7],
            group_binding: ptr::null(),
            expected_group_model: ptr::null(),
            expected_group_mode: 0,
            reserved2: 0,
            group_fields: ptr::null(),
            group_field_count: 0,
            reducers: ptr::null(),
            reducer_count: 0,
            reserved: [0; 4],
        }
    }

    fn function_argument(
        kind: u32,
        binding: *const TypeBridgeQueryBinding,
        value: *const TypeBridgeQueryFunctionValue,
        call: *const TypeBridgeQueryFunctionCall,
    ) -> TypeBridgeQueryFunctionArgumentV1 {
        TypeBridgeQueryFunctionArgumentV1 {
            struct_size: size_of::<TypeBridgeQueryFunctionArgumentV1>() as u32,
            version: DESCRIPTOR_VERSION,
            kind,
            reserved0: 0,
            binding,
            value,
            call,
            reserved: [0; 4],
        }
    }

    #[repr(C)]
    struct TestFunctionArgumentsV1 {
        header: TypeBridgeQueryFunctionArgumentsHeaderV1,
        argument: TypeBridgeQueryFunctionArgumentV1,
    }

    #[repr(C)]
    struct TestFunctionArgumentsMaxV1 {
        header: TypeBridgeQueryFunctionArgumentsHeaderV1,
        arguments: [TypeBridgeQueryFunctionArgumentV1; BOOLEAN_TERM_MAX],
    }

    fn function_arguments(argument: TypeBridgeQueryFunctionArgumentV1) -> TestFunctionArgumentsV1 {
        TestFunctionArgumentsV1 {
            header: TypeBridgeQueryFunctionArgumentsHeaderV1 {
                struct_size: size_of::<TestFunctionArgumentsV1>() as u32,
                version: DESCRIPTOR_VERSION,
                reserved: [0; 4],
            },
            argument,
        }
    }

    fn function_argument_member() -> TypeBridgeQueryFunctionArgumentMemberV1 {
        TypeBridgeQueryFunctionArgumentMemberV1 {
            struct_size: size_of::<TypeBridgeQueryFunctionArgumentMemberV1>() as u32,
            version: DESCRIPTOR_VERSION,
            args_offset: offset_of!(TestFunctionArgumentsV1, argument),
            reserved: [0; 4],
        }
    }

    fn function_arguments_graph(
        args: &TestFunctionArgumentsV1,
        members: &[TypeBridgeQueryFunctionArgumentMemberV1],
    ) -> TypeBridgeQueryFunctionArgumentsGraphV1 {
        TypeBridgeQueryFunctionArgumentsGraphV1 {
            struct_size: size_of::<TypeBridgeQueryFunctionArgumentsGraphV1>() as u32,
            version: DESCRIPTOR_VERSION,
            args: ptr::from_ref(args).cast(),
            args_size: size_of::<TestFunctionArgumentsV1>(),
            members: if members.is_empty() {
                ptr::null()
            } else {
                members.as_ptr()
            },
            member_count: members.len(),
            reserved: [0; 3],
        }
    }

    fn default_limits() -> TypeBridgeQueryExecutionLimitsV1 {
        TypeBridgeQueryExecutionLimitsV1 {
            struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
            version: DESCRIPTOR_VERSION,
            timeout_milliseconds: MAX_QUERY_TIMEOUT_MILLISECONDS,
            items: MAX_QUERY_ITEMS,
            bytes: MAX_QUERY_BYTES,
            graph_nodes: MAX_QUERY_GRAPH_NODES,
            attribute_values: MAX_QUERY_ATTRIBUTE_VALUES,
            collection_members: MAX_QUERY_COLLECTION_MEMBERS,
            role_players: MAX_QUERY_ROLE_PLAYERS,
            statements: MAX_QUERY_STATEMENTS,
            reserved0: 0,
            reserved: [0; 4],
        }
    }

    fn root_terminal_descriptor(
        built: &BuiltQuery,
        kind: u32,
    ) -> TypeBridgeQueryTerminalDescriptorV1 {
        let mut descriptor = terminal_descriptor(kind, ROWS_BOUNDED_MANY);
        descriptor.root = built.binding;
        descriptor.expected_root_model = &built.model;
        descriptor.expected_root_mode = MATCH_EXACT;
        descriptor
    }

    unsafe fn replace_terminal(
        built: &mut BuiltQuery,
        descriptor: &TypeBridgeQueryTerminalDescriptorV1,
    ) {
        assert_eq!(
            // SAFETY: fixture uniquely owns the old terminal slot.
            unsafe { type_bridge_query_terminal_close(&mut built.terminal) },
            TypeBridgeStatus::Ok,
        );
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: the descriptor graph and independent output slots remain live.
            unsafe {
                type_bridge_query_terminal_open_v1(
                    built.query,
                    descriptor,
                    &mut built.terminal,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
    }

    unsafe fn iid_predicate(
        built: &BuiltQuery,
        iid: &'static [u8],
    ) -> *mut TypeBridgeQueryPredicate {
        let mut predicate = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: exact generated handles and immutable IID bytes remain live.
            unsafe {
                type_bridge_query_binding_iid_v1(
                    built.binding,
                    &built.model,
                    MATCH_EXACT,
                    TypeBridgeByteView {
                        data: iid.as_ptr(),
                        length: iid.len(),
                    },
                    &mut predicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        predicate
    }

    #[test]
    fn provider_free_construction_rejects_inactive_junk_without_following_it() {
        let package = package("querylanes");
        // SAFETY: fixture owns all handles and exact generated tokens.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let mut descriptor = terminal_descriptor(TERMINAL_FIRST, ROWS_BOUNDED_MANY);
        descriptor.root = std::ptr::NonNull::<TypeBridgeQueryBinding>::dangling().as_ptr();
        let mut output = std::ptr::NonNull::<TypeBridgeQueryTerminal>::dangling().as_ptr();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: the inactive dangling lane must be rejected before traversal.
            unsafe {
                type_bridge_query_terminal_open_v1(
                    built.query,
                    &descriptor,
                    &mut output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(output.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_descriptor_layout_invalid"
        );
        // SAFETY: diagnostic owner is unique.
        unsafe { close_diagnostics(&mut diagnostics) };
    }

    #[test]
    fn every_query_opaque_family_close_is_same_slot_idempotent() {
        macro_rules! close_twice {
            ($close:path, $slot:expr) => {{
                assert_eq!(unsafe { $close($slot) }, TypeBridgeStatus::Ok);
                assert!((*$slot).is_null());
                assert_eq!(unsafe { $close($slot) }, TypeBridgeStatus::Ok);
                assert!((*$slot).is_null());
            }};
        }

        let package = package("querycloseall");
        // SAFETY: fixture owns every exact generated token and returned handle.
        let mut built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let person_id = TypeId::new(TypeKind::Entity, "person").expect("person identity");
        let identifier_field = projected_field_token(&package, &person_id, "identifier");
        let mut diagnostics = ptr::null_mut();
        let mut field = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_field_open(
                    built.binding,
                    &identifier_field,
                    &mut field,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut predicate = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_field_presence(
                    field,
                    &identifier_field,
                    1,
                    &mut predicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let order_descriptor = TypeBridgeQueryOrderDescriptorV1 {
            struct_size: size_of::<TypeBridgeQueryOrderDescriptorV1>() as u32,
            version: DESCRIPTOR_VERSION,
            field,
            expected_field: &identifier_field,
            direction: SORT_ASCENDING,
            missing: MISSING_REJECT,
            reserved0: 0,
            reserved: [0; 4],
        };
        let mut order = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_order_open_v1(&order_descriptor, &mut order, &mut diagnostics)
            },
            TypeBridgeStatus::Ok,
        );

        let membership_id =
            TypeId::new(TypeKind::Relation, "membership").expect("membership identity");
        let membership = model_token(&package, membership_id.clone());
        let member = projected_role_token(&package, &membership_id, "member");
        let mut relation_binding = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_binding_open_v1(
                    built.session,
                    &membership,
                    MATCH_EXACT,
                    &mut relation_binding,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut role = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_role_open(relation_binding, &member, &mut role, &mut diagnostics)
            },
            TypeBridgeStatus::Ok,
        );

        let identity = projected_function_token(&package, "identity-string");
        let mut function = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_open(
                    built.session,
                    &identity,
                    &mut function,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let attribute = model_token(
            &package,
            TypeId::new(TypeKind::Attribute, "identifier").expect("identifier attribute"),
        );
        let text = b"Ada";
        let mut raw_value = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_projected_value_string_open(
                    &package,
                    &attribute,
                    TypeBridgeByteView {
                        data: text.as_ptr(),
                        length: text.len(),
                    },
                    &mut raw_value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut function_value = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_value_open(
                    built.session,
                    raw_value,
                    &mut function_value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let function_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_VALUE,
            ptr::null(),
            function_value,
            ptr::null(),
        ));
        let member_table = [function_argument_member()];
        let graph = function_arguments_graph(&function_arguments, &member_table);
        let mut function_call = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    function,
                    &identity,
                    &graph,
                    &mut function_call,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let database = fake_database(&package, vec![Response::Items(Vec::new())], Vec::new());
        let mut result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let remote = remote_fixture(0x2f);
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let mut pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_FIRST) };
        let mut claim = unsafe { claim_remote(pending) };

        close_twice!(type_bridge_query_result_close, &mut result);
        close_twice!(type_bridge_query_remote_claim_close, &mut claim);
        close_twice!(type_bridge_query_remote_pending_close, &mut pending);
        close_twice!(type_bridge_query_remote_context_close, &mut context);
        close_twice!(type_bridge_query_function_call_close, &mut function_call);
        close_twice!(type_bridge_query_function_value_close, &mut function_value);
        close_twice!(type_bridge_query_function_close, &mut function);
        close_twice!(type_bridge_query_role_close, &mut role);
        close_twice!(type_bridge_query_binding_close, &mut relation_binding);
        close_twice!(type_bridge_query_order_close, &mut order);
        close_twice!(type_bridge_query_predicate_close, &mut predicate);
        close_twice!(type_bridge_query_field_close, &mut field);
        close_twice!(type_bridge_query_terminal_close, &mut built.terminal);
        close_twice!(type_bridge_query_close, &mut built.query);
        close_twice!(type_bridge_query_selection_close, &mut built.selection);
        close_twice!(type_bridge_query_binding_close, &mut built.binding);
        close_twice!(type_bridge_query_session_close, &mut built.session);
        assert_eq!(
            unsafe { type_bridge_projected_value_close(&mut raw_value) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
    }

    #[test]
    fn schema_function_calls_are_typed_composable_and_hostile_alias_safe() {
        let package_value = package("queryfunctions");
        let foreign = package("queryfunctionsforeign");
        // SAFETY: fixture owns exact generated handles and token storage.
        let built = unsafe { BuiltQuery::open(&package_value, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let identity = projected_function_token(&package_value, "identity-string");
        let person_identifier = projected_function_token(&package_value, "person-identifier");
        let identifier_field = projected_field_token(
            &package_value,
            &unsafe { &*built.binding }.model,
            "identifier",
        );
        let attribute = model_token(
            &package_value,
            TypeId::new(TypeKind::Attribute, "identifier").expect("identifier attribute"),
        );
        let mut diagnostics = ptr::null_mut();
        let mut identity_handle = ptr::null_mut();
        let mut person_handle = ptr::null_mut();
        let function_handle_failure = inject_failure(AllocationSite::QueryFunctionHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_function_open(
                    built.session,
                    &identity,
                    &mut identity_handle,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(identity_handle.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(function_handle_failure);
        assert_eq!(
            unsafe {
                type_bridge_query_function_open(
                    built.session,
                    &identity,
                    &mut identity_handle,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe {
                type_bridge_query_function_open(
                    built.session,
                    &person_identifier,
                    &mut person_handle,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let mut raw_value = ptr::null_mut();
        let text = b"Ada";
        assert_eq!(
            unsafe {
                type_bridge_projected_value_string_open(
                    &package_value,
                    &attribute,
                    TypeBridgeByteView {
                        data: text.as_ptr(),
                        length: text.len(),
                    },
                    &mut raw_value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut value = ptr::null_mut();
        let value_handle_failure = inject_failure(AllocationSite::QueryFunctionValueHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_function_value_open(
                    built.session,
                    raw_value,
                    &mut value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(value.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(value_handle_failure);
        assert_eq!(
            unsafe {
                type_bridge_query_function_value_open(
                    built.session,
                    raw_value,
                    &mut value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let value_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_VALUE,
            ptr::null(),
            value,
            ptr::null(),
        ));
        let single_member = [function_argument_member()];
        let value_graph = function_arguments_graph(&value_arguments, &single_member);
        let mut identity_call = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &value_graph,
                    &mut identity_call,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let binding_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_BINDING,
            built.binding,
            ptr::null(),
            ptr::null(),
        ));
        let binding_graph = function_arguments_graph(&binding_arguments, &single_member);
        let mut person_call = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    person_handle,
                    &person_identifier,
                    &binding_graph,
                    &mut person_call,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let nested_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_CALL,
            ptr::null(),
            ptr::null(),
            person_call,
        ));
        let nested_graph = function_arguments_graph(&nested_arguments, &single_member);
        let mut nested_call = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &nested_graph,
                    &mut nested_call,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let mut field = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_field_open(
                    built.binding,
                    &identifier_field,
                    &mut field,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut predicates = [ptr::null_mut(); 4];
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_compare_field(
                    identity_call,
                    COMPARE_EQUAL,
                    field,
                    &identifier_field,
                    &mut predicates[0],
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe {
                type_bridge_query_field_compare_function(
                    field,
                    &identifier_field,
                    COMPARE_EQUAL,
                    identity_call,
                    &mut predicates[1],
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_compare_value(
                    identity_call,
                    COMPARE_EQUAL,
                    value,
                    &mut predicates[2],
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_compare_call(
                    nested_call,
                    COMPARE_EQUAL,
                    person_call,
                    &mut predicates[3],
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        // Explicit casts cannot change the function identity fixed at open.
        let mut rejected = std::ptr::NonNull::<TypeBridgeQueryFunctionCall>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &person_identifier,
                    &value_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(rejected.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_nominal_contract_mismatch"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        // A valid tag with a populated inactive live-handle lane is rejected
        // canonically, but no inactive pointer is followed.
        let malformed_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_VALUE,
            built.binding,
            value,
            ptr::null(),
        ));
        let malformed_graph = function_arguments_graph(&malformed_arguments, &single_member);
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &malformed_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(rejected.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };

        // The first fallible argument reservation occurs only after the full
        // projected/package graph alias preflight. An output within an active
        // foreign package byte buffer therefore cannot be cleared, even when
        // that reservation is armed to fail.
        let mut foreign_session = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_session_open(&foreign, &mut foreign_session, &mut diagnostics)
            },
            TypeBridgeStatus::Ok,
        );
        let foreign_attribute = model_token(
            &foreign,
            TypeId::new(TypeKind::Attribute, "identifier").expect("foreign attribute"),
        );
        let mut foreign_raw = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_projected_value_string_open(
                    &foreign,
                    &foreign_attribute,
                    TypeBridgeByteView {
                        data: text.as_ptr(),
                        length: text.len(),
                    },
                    &mut foreign_raw,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut foreign_value = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_function_value_open(
                    foreign_session,
                    foreign_raw,
                    &mut foreign_value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let foreign_arguments = function_arguments(function_argument(
            FUNCTION_ARGUMENT_VALUE,
            ptr::null(),
            foreign_value,
            ptr::null(),
        ));
        let foreign_graph = function_arguments_graph(&foreign_arguments, &single_member);
        let sentinel = foreign.state.authority_json.clone();
        let aliased_output = foreign.state.authority_json.as_ptr().cast_mut().cast();
        let _failure = inject_failure(AllocationSite::QueryFunctionArguments, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &foreign_graph,
                    aliased_output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());
        assert_eq!(foreign.state.authority_json, sentinel);
        drop(_failure);

        // The bounded graph ceiling is fail-closed and read-only because an
        // over-cap table cannot be traversed to prove every nested alias.
        let mut over_limit_graph = value_graph;
        over_limit_graph.member_count = BOOLEAN_TERM_MAX + 1;
        let preserved_argument = value_arguments.argument;
        let over_limit_output = ptr::from_ref(&value_arguments.argument)
            .cast_mut()
            .cast::<*mut TypeBridgeQueryFunctionCall>();
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &over_limit_graph,
                    over_limit_output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(diagnostics.is_null());
        assert_eq!(value_arguments.argument.kind, preserved_argument.kind);
        assert_eq!(value_arguments.argument.value, preserved_argument.value);

        // The exact shared ceiling is fully walked from the hosted object and
        // static offset table. It reaches neutral signature validation (arity)
        // instead of being rejected by the C traversal budget.
        let repeated = function_argument(FUNCTION_ARGUMENT_VALUE, ptr::null(), value, ptr::null());
        let mut max_arguments = Box::new(TestFunctionArgumentsMaxV1 {
            header: TypeBridgeQueryFunctionArgumentsHeaderV1 {
                struct_size: size_of::<TestFunctionArgumentsMaxV1>() as u32,
                version: DESCRIPTOR_VERSION,
                reserved: [0; 4],
            },
            arguments: [repeated; BOOLEAN_TERM_MAX],
        });
        let mut max_members = Vec::with_capacity(BOOLEAN_TERM_MAX);
        for index in 0..BOOLEAN_TERM_MAX {
            max_members.push(TypeBridgeQueryFunctionArgumentMemberV1 {
                struct_size: size_of::<TypeBridgeQueryFunctionArgumentMemberV1>() as u32,
                version: DESCRIPTOR_VERSION,
                args_offset: offset_of!(TestFunctionArgumentsMaxV1, arguments)
                    + index * size_of::<TypeBridgeQueryFunctionArgumentV1>(),
                reserved: [0; 4],
            });
        }
        let max_graph = TypeBridgeQueryFunctionArgumentsGraphV1 {
            struct_size: size_of::<TypeBridgeQueryFunctionArgumentsGraphV1>() as u32,
            version: DESCRIPTOR_VERSION,
            args: ptr::from_mut(&mut *max_arguments).cast(),
            args_size: size_of::<TestFunctionArgumentsMaxV1>(),
            members: max_members.as_ptr(),
            member_count: max_members.len(),
            reserved: [0; 3],
        };
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &max_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(rejected.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "function_argument_arity"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        // Versioned generated argument objects are authenticated before their
        // common-layout nominal witnesses are interpreted.
        let mut bad_version_arguments = function_arguments(value_arguments.argument);
        bad_version_arguments.header.version = DESCRIPTOR_VERSION + 1;
        let bad_version_graph = function_arguments_graph(&bad_version_arguments, &single_member);
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &bad_version_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(rejected.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };

        // Both fallible reservations are retryable after the complete graph
        // and every active handle's borrowed buffers have been preflighted.
        let argument_failure = inject_failure(AllocationSite::QueryFunctionArguments, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &value_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(rejected.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(argument_failure);
        let call_failure = inject_failure(AllocationSite::QueryFunctionCallHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &value_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(rejected.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(call_failure);
        // The active foreign handle and local function remain reusable.
        assert_eq!(
            unsafe {
                type_bridge_query_function_call_open_v1(
                    identity_handle,
                    &identity,
                    &value_graph,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        assert_eq!(
            unsafe { type_bridge_query_function_call_close(&mut rejected) },
            TypeBridgeStatus::Ok,
        );
        for predicate in &mut predicates {
            assert_eq!(
                unsafe { type_bridge_query_predicate_close(predicate) },
                TypeBridgeStatus::Ok,
            );
        }
        for call in [&mut nested_call, &mut person_call, &mut identity_call] {
            assert_eq!(
                unsafe { type_bridge_query_function_call_close(call) },
                TypeBridgeStatus::Ok,
            );
        }
        assert_eq!(
            unsafe { type_bridge_query_field_close(&mut field) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_function_value_close(&mut foreign_value) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_projected_value_close(&mut foreign_raw) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_session_close(&mut foreign_session) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_function_value_close(&mut value) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_projected_value_close(&mut raw_value) },
            TypeBridgeStatus::Ok,
        );
        for function in [&mut person_handle, &mut identity_handle] {
            assert_eq!(
                unsafe { type_bridge_query_function_close(function) },
                TypeBridgeStatus::Ok,
            );
        }
    }

    #[test]
    fn role_and_reachability_nominal_casts_fail_read_only_before_valid_reuse() {
        let package = package("queryreachability");
        // SAFETY: fixture owns exact generated handles and copied tokens.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let membership_id =
            TypeId::new(TypeKind::Relation, "membership").expect("membership identity");
        let event_id = TypeId::new(TypeKind::Relation, "event").expect("event identity");
        let organization_id =
            TypeId::new(TypeKind::Entity, "organization").expect("organization identity");
        let membership = model_token(&package, membership_id.clone());
        let event = model_token(&package, event_id);
        let organization = model_token(&package, organization_id);
        let member = projected_role_token(&package, &membership_id, "member");

        let mut diagnostics = ptr::null_mut();
        let mut target = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_binding_open_v1(
                    built.session,
                    &event,
                    MATCH_EXACT,
                    &mut target,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut relation_binding = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_binding_open_v1(
                    built.session,
                    &membership,
                    MATCH_EXACT,
                    &mut relation_binding,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut role = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_role_open(relation_binding, &member, &mut role, &mut diagnostics)
            },
            TypeBridgeStatus::Ok,
        );

        // A cast to a different token kind cannot change which generated role
        // an already-opened role handle denotes.
        let mut predicate = std::ptr::NonNull::<TypeBridgeQueryPredicate>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_role_connects(
                    role,
                    &event,
                    built.binding,
                    &built.model,
                    MATCH_EXACT,
                    &mut predicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(predicate.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_projected_token_layout_invalid"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_role_connects(
                    role,
                    &member,
                    built.binding,
                    &built.model,
                    MATCH_EXACT,
                    &mut predicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_predicate_close(&mut predicate) },
            TypeBridgeStatus::Ok,
        );

        // Relation, endpoint model, and endpoint mode are all native parts of
        // the generated reachability witness. Every fabricated view must leave
        // both bindings and the session reusable.
        let fabricated = [
            (&event, &built.model, MATCH_EXACT, &event, MATCH_EXACT),
            (&membership, &organization, MATCH_EXACT, &event, MATCH_EXACT),
            (
                &membership,
                &built.model,
                MATCH_SUBTYPES,
                &event,
                MATCH_EXACT,
            ),
            (
                &membership,
                &built.model,
                MATCH_EXACT,
                &built.model,
                MATCH_EXACT,
            ),
            (
                &membership,
                &built.model,
                MATCH_EXACT,
                &event,
                MATCH_SUBTYPES,
            ),
        ];
        for (relation, source_model, source_mode, target_model, target_mode) in fabricated {
            predicate = std::ptr::NonNull::<TypeBridgeQueryPredicate>::dangling().as_ptr();
            assert_eq!(
                unsafe {
                    type_bridge_query_session_reachable(
                        built.session,
                        relation,
                        &member,
                        &member,
                        built.binding,
                        source_model,
                        source_mode,
                        target,
                        target_model,
                        target_mode,
                        1,
                        3,
                        &mut predicate,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::InvalidArgument,
            );
            assert!(predicate.is_null());
            assert_eq!(
                unsafe { diagnostic_code(diagnostics) },
                "c_query_nominal_contract_mismatch"
            );
            unsafe { close_diagnostics(&mut diagnostics) };
        }

        assert_eq!(
            unsafe {
                type_bridge_query_session_reachable(
                    built.session,
                    &membership,
                    &member,
                    &member,
                    built.binding,
                    &built.model,
                    MATCH_EXACT,
                    target,
                    &event,
                    MATCH_EXACT,
                    1,
                    3,
                    &mut predicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { type_bridge_query_predicate_close(&mut predicate) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_role_close(&mut role) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_binding_close(&mut relation_binding) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_binding_close(&mut target) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn malformed_boolean_topology_checks_live_outer_ranges_before_output_clear() {
        let package = package("queryboolalias");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        // SAFETY: helper retains fixture and copied static IID bytes.
        let mut left = unsafe { iid_predicate(&built, b"0x01") };
        let mut right = unsafe { iid_predicate(&built, b"0x02") };
        let mut diagnostics = ptr::null_mut();

        // Invalid tags still must not clear an output slot inside a live input.
        let interior_left =
            unsafe { left.cast::<u8>().add(1) }.cast::<*mut TypeBridgeQueryPredicate>();
        assert_eq!(
            unsafe {
                type_bridge_query_predicate_combine(
                    u32::MAX,
                    left,
                    right,
                    interior_left,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());

        // NOT's right lane is inactive semantically, but a non-null live outer
        // object still protects its complete storage from output corruption.
        let interior_right =
            unsafe { right.cast::<u8>().add(1) }.cast::<*mut TypeBridgeQueryPredicate>();
        assert_eq!(
            unsafe {
                type_bridge_query_predicate_combine(
                    PREDICATE_NOT,
                    left,
                    right,
                    interior_right,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());

        // Both inputs remain reusable after each hostile call.
        let mut combined = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_predicate_combine(
                    PREDICATE_AND,
                    left,
                    right,
                    &mut combined,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_predicate_close(&mut combined) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_predicate_close(&mut right) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_predicate_close(&mut left) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn named_shapes_copy_validate_deduplicate_and_bind_names_into_shape_identity() {
        let package = package("querynames");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let mut diagnostics = ptr::null_mut();
        let mut slot = TypeBridgeQueryShapeSlotV1 {
            struct_size: size_of::<TypeBridgeQueryShapeSlotV1>() as u32,
            version: DESCRIPTOR_VERSION,
            selection: built.selection,
            expected_model: &built.model,
            expected_mode: MATCH_EXACT,
            expected_kind: SELECTION_ONE,
            reserved0: 0,
            name: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            reserved: [0; 4],
        };
        let mut descriptor = TypeBridgeQueryDescriptorV1 {
            struct_size: size_of::<TypeBridgeQueryDescriptorV1>() as u32,
            version: DESCRIPTOR_VERSION,
            shape_kind: SHAPE_NAMED,
            reserved0: 0,
            slots: &slot,
            slot_count: 1,
            reserved: [0; 4],
        };

        let invalid_utf8 = [0xff];
        slot.name = TypeBridgeByteView {
            data: invalid_utf8.as_ptr(),
            length: invalid_utf8.len(),
        };
        let mut query = std::ptr::NonNull::<TypeBridgeQuery>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(built.session, &descriptor, &mut query, &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(query.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_descriptor_layout_invalid"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let control_name = b"bad\nname";
        slot.name = TypeBridgeByteView {
            data: control_name.as_ptr(),
            length: control_name.len(),
        };
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(built.session, &descriptor, &mut query, &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(query.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "invalid_output_name"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let primary_name = b"person";
        slot.name = TypeBridgeByteView {
            data: primary_name.as_ptr(),
            length: primary_name.len(),
        };
        let mut primary_query = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(
                    built.session,
                    &descriptor,
                    &mut primary_query,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );

        let mut renamed = b"renamed_person".to_vec();
        slot.name = TypeBridgeByteView {
            data: renamed.as_ptr(),
            length: renamed.len(),
        };
        let mut renamed_query = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(
                    built.session,
                    &descriptor,
                    &mut renamed_query,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        renamed.fill(b'x');
        let original_renamed = b"renamed_person";
        slot.name = TypeBridgeByteView {
            data: original_renamed.as_ptr(),
            length: original_renamed.len(),
        };
        let mut copied_reference_query = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(
                    built.session,
                    &descriptor,
                    &mut copied_reference_query,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: both immutable C handles remain retained in this test.
        let primary_request = unsafe { &*primary_query }
            .handle
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::BoundedMany,
            )
            .expect("primary named request validates");
        // SAFETY: both immutable C handles remain retained in this test.
        let renamed_request = unsafe { &*renamed_query }
            .handle
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::BoundedMany,
            )
            .expect("renamed request validates");
        assert_ne!(primary_request.shape_id(), renamed_request.shape_id());
        // SAFETY: the reference query is retained and was opened from the
        // original bytes after the caller buffer backing renamed_query changed.
        let copied_reference_request = unsafe { &*copied_reference_query }
            .handle
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::BoundedMany,
            )
            .expect("copied-name reference request validates");
        assert_eq!(
            renamed_request.shape_id(),
            copied_reference_request.shape_id()
        );

        let mut second_binding = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_binding_open_v1(
                    built.session,
                    &built.model,
                    MATCH_EXACT,
                    &mut second_binding,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let second_descriptor = TypeBridgeQuerySelectionDescriptorV1 {
            struct_size: size_of::<TypeBridgeQuerySelectionDescriptorV1>() as u32,
            version: DESCRIPTOR_VERSION,
            binding: second_binding,
            expected_model: &built.model,
            expected_mode: MATCH_EXACT,
            kind: SELECTION_ONE,
            distinct: 0,
            reserved0: [0; 7],
            orders: ptr::null(),
            order_count: 0,
            reserved: [0; 4],
        };
        let mut second_selection = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_selection_open_v1(
                    &second_descriptor,
                    &mut second_selection,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let duplicate_name = b"duplicate";
        let duplicate_slots = [
            TypeBridgeQueryShapeSlotV1 {
                name: TypeBridgeByteView {
                    data: duplicate_name.as_ptr(),
                    length: duplicate_name.len(),
                },
                ..slot
            },
            TypeBridgeQueryShapeSlotV1 {
                selection: second_selection,
                name: TypeBridgeByteView {
                    data: duplicate_name.as_ptr(),
                    length: duplicate_name.len(),
                },
                ..slot
            },
        ];
        descriptor.slots = duplicate_slots.as_ptr();
        descriptor.slot_count = duplicate_slots.len();
        query = std::ptr::NonNull::<TypeBridgeQuery>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_open_v1(built.session, &descriptor, &mut query, &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(query.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "duplicate_output_name"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_query_close(&mut copied_reference_query) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_close(&mut renamed_query) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_close(&mut primary_query) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_selection_close(&mut second_selection) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_binding_close(&mut second_binding) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn direct_exact_one_matrix_materializes_once_and_fences_child_aliases() {
        for (rows, expected_code) in [(0, Some("no_result")), (1, None), (2, Some("not_unique"))] {
            let mut package = Box::into_raw(Box::new(package("queryexact")));
            // SAFETY: package pointer stays live through construction and database branding.
            let mut built = unsafe { BuiltQuery::open(&*package, TERMINAL_ROWS, ROWS_EXACTLY_ONE) };
            let wrong_model = model_token(
                // SAFETY: package remains live until after all generated tokens are copied.
                unsafe { &*package },
                TypeId::new(TypeKind::Entity, "organization").expect("test organization type"),
            );
            let solutions = (0..rows)
                .map(|index| solution(if index == 0 { "0x01" } else { "0x02" }))
                .collect();
            let hydrated = (0..rows)
                .map(|index| {
                    hydration(
                        unsafe { &*package },
                        if index == 0 { "0x01" } else { "0x02" },
                        if index == 0 { "Ada" } else { "Bob" },
                    )
                })
                .collect();
            // SAFETY: package is live and shares the exact retained Arc lineage.
            let database = unsafe {
                fake_database(
                    &*package,
                    vec![Response::Items(solutions)],
                    vec![Response::Items(hydrated)],
                )
            };
            // Terminal descendants retain the package and session after their public parents close.
            // SAFETY: builder handles remain uniquely owned.
            unsafe { built.close_builders() };
            assert_eq!(
                // SAFETY: package slot uniquely owns this public package handle.
                unsafe { type_bridge_schema_package_close(&mut package) },
                TypeBridgeStatus::Ok,
            );
            let mut result = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            let status = unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            };
            if let Some(code) = expected_code {
                assert_eq!(status, TypeBridgeStatus::InvalidArgument);
                assert!(result.is_null());
                assert_eq!(unsafe { diagnostic_code(diagnostics) }, code);
                // SAFETY: diagnostics owner is unique.
                unsafe { close_diagnostics(&mut diagnostics) };
            } else {
                let failure_code =
                    (!diagnostics.is_null()).then(|| unsafe { diagnostic_code(diagnostics) });
                assert_eq!(status, TypeBridgeStatus::Ok, "{failure_code:?}");
                assert!(diagnostics.is_null());
                let mut count = usize::MAX;
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_ROWS,
                            &mut count,
                            ptr::null_mut(),
                        )
                    },
                    TypeBridgeStatus::InvalidArgument,
                );
                assert_eq!(count, 0);
                diagnostics =
                    std::ptr::NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_ROWS,
                            ptr::null_mut(),
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::InvalidArgument,
                );
                assert!(diagnostics.is_null());
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_ROWS,
                            &mut count,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok,
                );
                assert_eq!(count, 1);

                // A fabricated Rows-as-Page result view fails atomically, then the
                // same immutable result remains reusable with its exact kind.
                count = usize::MAX;
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_PAGE,
                            &mut count,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::ExecutionFailed,
                );
                assert_eq!(count, 0);
                assert_eq!(
                    unsafe { diagnostic_code(diagnostics) },
                    "c_query_result_contract_mismatch"
                );
                unsafe { close_diagnostics(&mut diagnostics) };

                let invalid_slot_contracts = [
                    (
                        1,
                        &built.model,
                        MATCH_EXACT,
                        SELECTION_ONE,
                        TypeBridgeStatus::InvalidArgument,
                        "c_query_result_index_invalid",
                    ),
                    (
                        0,
                        &wrong_model,
                        MATCH_EXACT,
                        SELECTION_ONE,
                        TypeBridgeStatus::ExecutionFailed,
                        "c_query_result_contract_mismatch",
                    ),
                    (
                        0,
                        &built.model,
                        MATCH_SUBTYPES,
                        SELECTION_ONE,
                        TypeBridgeStatus::ExecutionFailed,
                        "c_query_result_contract_mismatch",
                    ),
                    (
                        0,
                        &built.model,
                        MATCH_EXACT,
                        SELECTION_COLLECT,
                        TypeBridgeStatus::ExecutionFailed,
                        "c_query_result_contract_mismatch",
                    ),
                ];
                for (slot_index, model, mode, selection_kind, expected_status, expected_code) in
                    invalid_slot_contracts
                {
                    let mut slot_count = usize::MAX;
                    assert_eq!(
                        unsafe {
                            type_bridge_query_result_row_slot_count(
                                result,
                                RESULT_ROWS,
                                0,
                                slot_index,
                                model,
                                mode,
                                selection_kind,
                                &mut slot_count,
                                &mut diagnostics,
                            )
                        },
                        expected_status,
                    );
                    assert_eq!(slot_count, 0);
                    assert_eq!(unsafe { diagnostic_code(diagnostics) }, expected_code);
                    unsafe { close_diagnostics(&mut diagnostics) };
                }

                let mut slot_count = usize::MAX;
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_slot_count(
                            result,
                            RESULT_ROWS,
                            0,
                            0,
                            &built.model,
                            MATCH_EXACT,
                            SELECTION_ONE,
                            &mut slot_count,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok,
                );
                assert_eq!(slot_count, 1);

                // The thing reader owns the same native expectation fence and
                // independently clears a hostile stale output on mismatch.
                let mut thing = std::ptr::NonNull::<TypeBridgeProjectedThing>::dangling().as_ptr();
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_slot_thing_at(
                            result,
                            RESULT_ROWS,
                            0,
                            0,
                            0,
                            &wrong_model,
                            MATCH_EXACT,
                            SELECTION_ONE,
                            &mut thing,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::ExecutionFailed,
                );
                assert!(thing.is_null());
                unsafe { close_diagnostics(&mut diagnostics) };

                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_slot_thing_at(
                            result,
                            RESULT_ROWS,
                            0,
                            0,
                            0,
                            &built.model,
                            MATCH_EXACT,
                            SELECTION_ONE,
                            &mut thing,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok,
                );
                let mut iid = TypeBridgeByteView {
                    data: ptr::null(),
                    length: 0,
                };
                assert_eq!(
                    unsafe { type_bridge_projected_thing_iid(thing, &mut iid) },
                    TypeBridgeStatus::Ok,
                );
                assert!(iid.length >= 4);
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_ROWS,
                            iid.data.add(1).cast_mut().cast::<usize>(),
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::InvalidArgument,
                );
                assert!(diagnostics.is_null());
                assert_eq!(
                    unsafe {
                        type_bridge_query_result_row_count(
                            result,
                            RESULT_ROWS,
                            &mut count,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok,
                );
                assert_eq!(
                    unsafe { type_bridge_projected_thing_close(&mut thing) },
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    unsafe { type_bridge_query_result_close(&mut result) },
                    TypeBridgeStatus::Ok
                );
            }
            assert_eq!(database.events.opens.load(Ordering::Acquire), 1);
            assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn first_zero_one_and_longer_stream_publish_at_most_one_row() {
        for rows in 0..=2 {
            let package = package("queryfirst");
            // SAFETY: fixture owns exact generated handles.
            let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
            let solutions = (0..rows)
                .map(|index| solution(if index == 0 { "0x01" } else { "0x02" }))
                .collect();
            let hydrations = (rows != 0)
                .then(|| Response::Items(vec![hydration(&package, "0x01", "Ada")]))
                .into_iter()
                .collect();
            let database = fake_database(&package, vec![Response::Items(solutions)], hydrations);
            let mut result = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            assert_eq!(
                // SAFETY: retained branded handles and disjoint writable outputs are live.
                unsafe {
                    type_bridge_database_query_execute_v1(
                        &*database.value,
                        built.terminal,
                        TERMINAL_FIRST,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut count = usize::MAX;
            assert_eq!(
                // SAFETY: result and outputs remain live and disjoint.
                unsafe {
                    type_bridge_query_result_row_count(
                        result,
                        RESULT_ROWS,
                        &mut count,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(count, usize::from(rows != 0));
            assert_eq!(database.events.queries.load(Ordering::Acquire), 1);
            assert_eq!(
                database.events.hydrations.load(Ordering::Acquire),
                usize::from(rows != 0),
            );
            assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
            assert_eq!(
                // SAFETY: fixture uniquely owns the result slot.
                unsafe { type_bridge_query_result_close(&mut result) },
                TypeBridgeStatus::Ok,
            );
        }
    }

    #[test]
    fn page_count_exists_and_ungrouped_reduction_have_closed_readers() {
        let package = package("queryterminals");

        // Page owns a same-transaction distinct total, selection, and re-match.
        // SAFETY: fixture owns exact generated handles.
        let mut page = unsafe { BuiltQuery::open(&package, TERMINAL_PAGE, ROWS_BOUNDED_MANY) };
        let mut page_descriptor = root_terminal_descriptor(&page, TERMINAL_PAGE);
        page_descriptor.include_total = 1;
        // SAFETY: the descriptor borrows only retained fixture handles for this call.
        unsafe { replace_terminal(&mut page, &page_descriptor) };
        let page_database = fake_database(
            &package,
            vec![
                Response::Items(vec![solution("0x01"), solution("0x01"), solution("0x02")]),
                Response::Items(vec![solution("0x02")]),
            ],
            vec![Response::Items(vec![rematch(&package, "0x02", "Bob")])],
        );
        let mut page_result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let status = unsafe {
            type_bridge_database_query_execute_v1(
                &*page_database.value,
                page.terminal,
                TERMINAL_PAGE,
                ptr::null(),
                ptr::null(),
                &mut page_result,
                &mut diagnostics,
            )
        };
        let failure_code =
            (!diagnostics.is_null()).then(|| unsafe { diagnostic_code(diagnostics) });
        assert_eq!(status, TypeBridgeStatus::Ok, "{failure_code:?}");
        assert_eq!(
            unsafe { type_bridge_query_result_kind(page_result, ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument,
        );
        let mut result_kind = u32::MAX;
        assert_eq!(
            unsafe { type_bridge_query_result_kind(page_result, &mut result_kind) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(result_kind, RESULT_PAGE);
        let mut page_rows = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_count(
                    page_result,
                    RESULT_PAGE,
                    &mut page_rows,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(page_rows, 1);

        page_rows = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_count(
                    page_result,
                    RESULT_ROWS,
                    &mut page_rows,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(page_rows, 0);
        unsafe { close_diagnostics(&mut diagnostics) };
        let mut page_slot_count = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_slot_count(
                    page_result,
                    RESULT_PAGE,
                    0,
                    0,
                    &page.model,
                    MATCH_EXACT,
                    SELECTION_COLLECT,
                    &mut page_slot_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(page_slot_count, 0);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_slot_count(
                    page_result,
                    RESULT_PAGE,
                    0,
                    0,
                    &page.model,
                    MATCH_EXACT,
                    SELECTION_ONE,
                    &mut page_slot_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(page_slot_count, 1);

        let mut metadata = std::mem::MaybeUninit::<TypeBridgeQueryPageMetadataV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_query_result_page_metadata_v1(
                    page_result,
                    metadata.as_mut_ptr(),
                    ptr::null_mut(),
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        // SAFETY: the independent-output contract initializes this slot even
        // when the required diagnostics output is absent.
        let cleared_metadata = unsafe { metadata.assume_init() };
        assert_eq!(cleared_metadata.struct_size, 0);
        assert_eq!(cleared_metadata.version, 0);
        diagnostics = std::ptr::NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_result_page_metadata_v1(
                    page_result,
                    ptr::null_mut(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());
        let mut metadata = std::mem::MaybeUninit::<TypeBridgeQueryPageMetadataV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_query_result_page_metadata_v1(
                    page_result,
                    metadata.as_mut_ptr(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: the successful reader initialized the complete metadata value.
        let metadata = unsafe { metadata.assume_init() };
        assert_eq!((metadata.offset, metadata.limit), (0, 1));
        assert_eq!((metadata.has_total, metadata.total), (1, 2));
        assert_eq!(page_database.events.queries.load(Ordering::Acquire), 2);
        assert_eq!(page_database.events.hydrations.load(Ordering::Acquire), 1);
        assert_eq!(page_database.events.closes.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut page_result) },
            TypeBridgeStatus::Ok,
        );

        // Count deduplicates root identities and never hydrates.
        // SAFETY: fixture owns exact generated handles.
        let count = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let count_database = fake_database(
            &package,
            vec![Response::Items(vec![
                solution("0x01"),
                solution("0x01"),
                solution("0x02"),
            ])],
            Vec::new(),
        );
        let mut count_result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*count_database.value,
                    count.terminal,
                    TERMINAL_COUNT,
                    ptr::null(),
                    ptr::null(),
                    &mut count_result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut count_value = u64::MAX;
        assert_eq!(
            unsafe {
                type_bridge_query_result_count(count_result, &mut count_value, ptr::null_mut())
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert_eq!(count_value, 0);
        diagnostics = std::ptr::NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_result_count(count_result, ptr::null_mut(), &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe {
                type_bridge_query_result_count(count_result, &mut count_value, &mut diagnostics)
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(count_value, 2);
        assert_eq!(count_database.events.hydrations.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut count_result) },
            TypeBridgeStatus::Ok,
        );

        for (items, expected) in [(Vec::new(), 0), (vec![solution("0x01")], 1)] {
            // SAFETY: fixture owns exact generated handles.
            let exists = unsafe { BuiltQuery::open(&package, TERMINAL_EXISTS, ROWS_BOUNDED_MANY) };
            let database = fake_database(&package, vec![Response::Items(items)], Vec::new());
            let mut result = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_database_query_execute_v1(
                        &*database.value,
                        exists.terminal,
                        TERMINAL_EXISTS,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut value = u8::MAX;
            assert_eq!(
                unsafe { type_bridge_query_result_exists(result, &mut value, &mut diagnostics) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(value, expected);
            assert_eq!(database.events.hydrations.load(Ordering::Acquire), 0);
            assert_eq!(
                unsafe { type_bridge_query_result_close(&mut result) },
                TypeBridgeStatus::Ok,
            );
        }

        // SAFETY: fixture owns exact generated handles.
        let reduction = unsafe { BuiltQuery::open(&package, TERMINAL_REDUCE, ROWS_BOUNDED_MANY) };
        let reduction_database = fake_database(
            &package,
            vec![Response::Items(vec![
                solution("0x01"),
                solution("0x01"),
                solution("0x02"),
            ])],
            Vec::new(),
        );
        let mut reduction_result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*reduction_database.value,
                    reduction.terminal,
                    TERMINAL_REDUCE,
                    ptr::null(),
                    ptr::null(),
                    &mut reduction_result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut reduction_rows = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_row_count(
                    reduction_result,
                    RESULT_REDUCTION,
                    &mut reduction_rows,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(reduction_rows, 1);
        let mut group_kind = u32::MAX;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_kind(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    &mut group_kind,
                    ptr::null_mut(),
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert_eq!(group_kind, GROUP_NONE);
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_kind(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    &mut group_kind,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(group_kind, GROUP_NONE);
        let mut reduced_metadata =
            std::mem::MaybeUninit::<TypeBridgeQueryReducedValueMetadataV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_value_metadata_v1(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    0,
                    REDUCED_COUNT,
                    reduced_metadata.as_mut_ptr(),
                    ptr::null_mut(),
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        // SAFETY: the independent-output contract initialized the full value.
        let cleared_metadata = unsafe { reduced_metadata.assume_init() };
        assert_eq!(cleared_metadata.struct_size, 0);
        assert_eq!(cleared_metadata.kind, 0);
        let mut reduced_metadata =
            std::mem::MaybeUninit::<TypeBridgeQueryReducedValueMetadataV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_value_metadata_v1(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    0,
                    REDUCED_COUNT,
                    reduced_metadata.as_mut_ptr(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: successful metadata read initialized the complete value.
        let reduced_metadata = unsafe { reduced_metadata.assume_init() };
        assert_eq!(reduced_metadata.kind, REDUCED_COUNT);
        assert_eq!(reduced_metadata.present, 1);
        let mut reduced = u64::MAX;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_value_count(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    0,
                    &mut reduced,
                    ptr::null_mut(),
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert_eq!(reduced, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_value_count(
                    reduction_result,
                    RESULT_REDUCTION,
                    0,
                    0,
                    &mut reduced,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(reduced, 2);
        assert_eq!(
            reduction_database.events.hydrations.load(Ordering::Acquire),
            0
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut reduction_result) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn scalar_and_tuple_field_reductions_preserve_exact_group_contracts() {
        for tuple in [false, true] {
            let package = package(if tuple {
                "querytuplegroup"
            } else {
                "queryfieldgroup"
            });
            // Start from a valid ungrouped reducer, then replace it after opening fields.
            // SAFETY: fixture owns exact generated handles.
            let mut built =
                unsafe { BuiltQuery::open(&package, TERMINAL_REDUCE, ROWS_BOUNDED_MANY) };
            let person = TypeId::new(TypeKind::Entity, "person").expect("test person type");
            let department_token = projected_field_token(&package, &person, "department");
            let identifier_token = projected_field_token(&package, &person, "identifier");
            let mut department = ptr::null_mut();
            let mut identifier = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_query_field_open(
                        built.binding,
                        &department_token,
                        &mut department,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe {
                    type_bridge_query_field_open(
                        built.binding,
                        &identifier_token,
                        &mut identifier,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let field_refs = [
                TypeBridgeQueryFieldReferenceV1 {
                    struct_size: size_of::<TypeBridgeQueryFieldReferenceV1>() as u32,
                    version: DESCRIPTOR_VERSION,
                    field: identifier,
                    expected_field: &identifier_token,
                    reserved: [0; 4],
                },
                TypeBridgeQueryFieldReferenceV1 {
                    struct_size: size_of::<TypeBridgeQueryFieldReferenceV1>() as u32,
                    version: DESCRIPTOR_VERSION,
                    field: department,
                    expected_field: &department_token,
                    reserved: [0; 4],
                },
            ];
            let reducer = TypeBridgeQueryReducerV1 {
                struct_size: size_of::<TypeBridgeQueryReducerV1>() as u32,
                version: DESCRIPTOR_VERSION,
                kind: REDUCER_COUNT,
                reserved0: 0,
                input: ptr::null(),
                expected_field: ptr::null(),
                reserved: [0; 4],
            };
            let terminal_kind = if tuple {
                TERMINAL_REDUCE_FIELDS
            } else {
                TERMINAL_REDUCE_FIELD
            };
            let expected_result_kind = if tuple {
                RESULT_FIELD_TUPLE_REDUCTION
            } else {
                RESULT_FIELD_REDUCTION
            };
            let active_refs = if tuple {
                &field_refs[..]
            } else {
                &field_refs[1..]
            };
            let mut descriptor = root_terminal_descriptor(&built, terminal_kind);
            descriptor.group_fields = active_refs.as_ptr();
            descriptor.group_field_count = active_refs.len();
            descriptor.reducers = &reducer;
            descriptor.reducer_count = 1;
            // SAFETY: descriptor graph remains live for the complete provider-free open call.
            unsafe { replace_terminal(&mut built, &descriptor) };

            let root_items = vec![solution("0x01"), solution("0x02")];
            let rematches = vec![
                rematch_bindings(vec![hydrated_thing_value(
                    &package,
                    0,
                    "person",
                    "0x01",
                    &[("identifier", "Ada"), ("department", "Engineering")],
                )]),
                rematch_bindings(vec![hydrated_thing_value(
                    &package,
                    0,
                    "person",
                    "0x02",
                    &[("identifier", "Bob"), ("department", "Engineering")],
                )]),
            ];
            let database = fake_database(
                &package,
                vec![Response::Items(root_items)],
                vec![Response::Items(rematches)],
            );
            let mut result = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_database_query_execute_v1(
                        &*database.value,
                        built.terminal,
                        terminal_kind,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut rows = usize::MAX;
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_row_count(
                        result,
                        expected_result_kind,
                        &mut rows,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(rows, if tuple { 2 } else { 1 });
            let mut kind = u32::MAX;
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_group_kind(
                        result,
                        expected_result_kind,
                        0,
                        &mut kind,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(kind, if tuple { GROUP_FIELDS } else { GROUP_FIELD });
            let mut field_count = 0;
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_group_field_count(
                        result,
                        expected_result_kind,
                        0,
                        &mut field_count,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(field_count, active_refs.len());
            let field_index = usize::from(tuple);
            let mut value = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_group_field_at(
                        result,
                        expected_result_kind,
                        0,
                        field_index,
                        &department_token,
                        &mut value,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut text = TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            };
            assert_eq!(
                unsafe { type_bridge_projected_value_text(value, &mut text) },
                TypeBridgeStatus::Ok,
            );
            // SAFETY: text borrows the retained projected value.
            assert_eq!(
                unsafe { std::slice::from_raw_parts(text.data, text.length) },
                b"Engineering",
            );

            // A same-domain field-handle cast cannot relabel the retained group
            // contract; the failed clone leaves the immutable result reusable.
            let mut mismatched_value =
                std::ptr::NonNull::<TypeBridgeProjectedValue>::dangling().as_ptr();
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_group_field_at(
                        result,
                        expected_result_kind,
                        0,
                        field_index,
                        &identifier_token,
                        &mut mismatched_value,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed,
            );
            assert!(mismatched_value.is_null());
            assert_eq!(
                unsafe { diagnostic_code(diagnostics) },
                "c_query_result_contract_mismatch"
            );
            unsafe { close_diagnostics(&mut diagnostics) };

            let mut metadata =
                std::mem::MaybeUninit::<TypeBridgeQueryReducedValueMetadataV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_value_metadata_v1(
                        result,
                        expected_result_kind,
                        0,
                        0,
                        REDUCED_LONG,
                        metadata.as_mut_ptr(),
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed,
            );
            // SAFETY: failure independently initialized the metadata output.
            assert_eq!(unsafe { metadata.assume_init() }.kind, 0);
            unsafe { close_diagnostics(&mut diagnostics) };

            let mut reduced = u64::MAX;
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_value_count(
                        result,
                        expected_result_kind,
                        0,
                        0,
                        &mut reduced,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(reduced, if tuple { 1 } else { 2 });

            // A cast between the two reduction shapes is fenced in the same native call.
            rows = usize::MAX;
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_row_count(
                        result,
                        if tuple {
                            RESULT_FIELD_REDUCTION
                        } else {
                            RESULT_FIELD_TUPLE_REDUCTION
                        },
                        &mut rows,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed,
            );
            assert_eq!(rows, 0);
            assert_eq!(
                unsafe { diagnostic_code(diagnostics) },
                "c_query_result_contract_mismatch"
            );
            unsafe { close_diagnostics(&mut diagnostics) };
            assert_eq!(
                unsafe {
                    type_bridge_query_result_reduction_row_count(
                        result,
                        expected_result_kind,
                        &mut rows,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(rows, if tuple { 2 } else { 1 });
            assert_eq!(
                unsafe { type_bridge_projected_value_close(&mut value) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_result_close(&mut result) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_field_close(&mut identifier) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_field_close(&mut department) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn binding_grouped_reduction_clones_only_fenced_group_things() {
        let package = package("querybindinggroup");
        // SAFETY: fixture owns exact generated handles.
        let mut built = unsafe { BuiltQuery::open(&package, TERMINAL_REDUCE, ROWS_BOUNDED_MANY) };
        let organization_model = model_token(
            &package,
            TypeId::new(TypeKind::Entity, "organization").expect("test organization type"),
        );
        let mut organization = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_binding_open_v1(
                    built.session,
                    &organization_model,
                    MATCH_EXACT,
                    &mut organization,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut query = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_add_hidden(
                    built.query,
                    organization,
                    &organization_model,
                    MATCH_EXACT,
                    &mut query,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_close(&mut built.query) },
            TypeBridgeStatus::Ok,
        );
        built.query = query;
        query = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_allow_cross_join(
                    built.query,
                    built.binding,
                    &built.model,
                    MATCH_EXACT,
                    organization,
                    &organization_model,
                    MATCH_EXACT,
                    &mut query,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_close(&mut built.query) },
            TypeBridgeStatus::Ok,
        );
        built.query = query;
        let reducer = TypeBridgeQueryReducerV1 {
            struct_size: size_of::<TypeBridgeQueryReducerV1>() as u32,
            version: DESCRIPTOR_VERSION,
            kind: REDUCER_COUNT,
            reserved0: 0,
            input: ptr::null(),
            expected_field: ptr::null(),
            reserved: [0; 4],
        };
        let mut descriptor = root_terminal_descriptor(&built, TERMINAL_REDUCE);
        descriptor.group_binding = organization;
        descriptor.expected_group_model = &organization_model;
        descriptor.expected_group_mode = MATCH_EXACT;
        descriptor.reducers = &reducer;
        descriptor.reducer_count = 1;
        // SAFETY: descriptor graph is retained for provider-free terminal opening.
        unsafe { replace_terminal(&mut built, &descriptor) };

        let database = fake_database(
            &package,
            vec![Response::Items(vec![
                solution("0x01"),
                solution("0x02"),
                solution("0x03"),
            ])],
            vec![Response::Items(vec![
                rematch_bindings(vec![
                    hydrated_thing_value(&package, 0, "person", "0x01", &[("identifier", "Ada")]),
                    hydrated_thing_value(
                        &package,
                        1,
                        "organization",
                        "0x11",
                        &[("identifier", "Acme")],
                    ),
                ]),
                rematch_bindings(vec![
                    hydrated_thing_value(&package, 0, "person", "0x02", &[("identifier", "Bob")]),
                    hydrated_thing_value(
                        &package,
                        1,
                        "organization",
                        "0x11",
                        &[("identifier", "Acme")],
                    ),
                ]),
                rematch_bindings(vec![
                    hydrated_thing_value(&package, 0, "person", "0x03", &[("identifier", "Cleo")]),
                    hydrated_thing_value(
                        &package,
                        1,
                        "organization",
                        "0x12",
                        &[("identifier", "Beta")],
                    ),
                ]),
            ])],
        );
        let mut result = ptr::null_mut();
        let status = unsafe {
            type_bridge_database_query_execute_v1(
                &*database.value,
                built.terminal,
                TERMINAL_REDUCE,
                ptr::null(),
                ptr::null(),
                &mut result,
                &mut diagnostics,
            )
        };
        let failure_code =
            (!diagnostics.is_null()).then(|| unsafe { diagnostic_code(diagnostics) });
        assert_eq!(status, TypeBridgeStatus::Ok, "{failure_code:?}");
        let mut rows = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_row_count(
                    result,
                    RESULT_REDUCTION,
                    &mut rows,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(rows, 2);
        let mut group_kind = GROUP_NONE;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_kind(
                    result,
                    RESULT_REDUCTION,
                    0,
                    &mut group_kind,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(group_kind, GROUP_THING);
        let mut thing = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_thing(
                    result,
                    RESULT_REDUCTION,
                    0,
                    &organization_model,
                    MATCH_EXACT,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut iid = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            unsafe { type_bridge_projected_thing_iid(thing, &mut iid) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: IID borrows the retained group thing handle.
        assert_eq!(
            unsafe { std::slice::from_raw_parts(iid.data, iid.length) },
            b"0x11"
        );
        let mut mismatched_thing =
            std::ptr::NonNull::<TypeBridgeProjectedThing>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_thing(
                    result,
                    RESULT_REDUCTION,
                    0,
                    &built.model,
                    MATCH_EXACT,
                    &mut mismatched_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(mismatched_thing.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_thing(
                    result,
                    RESULT_REDUCTION,
                    0,
                    &organization_model,
                    MATCH_SUBTYPES,
                    &mut mismatched_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(mismatched_thing.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_thing(
                    result,
                    RESULT_REDUCTION,
                    0,
                    &organization_model,
                    MATCH_EXACT,
                    &mut mismatched_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_projected_thing_close(&mut mismatched_thing) },
            TypeBridgeStatus::Ok,
        );
        let mut reduced = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_value_count(
                    result,
                    RESULT_REDUCTION,
                    0,
                    0,
                    &mut reduced,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(reduced, 2);
        assert_eq!(
            unsafe { type_bridge_projected_thing_close(&mut thing) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_binding_close(&mut organization) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn result_and_child_handle_allocation_failures_are_retryable_and_close_owned_context() {
        let result_package = package("queryalloc");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&result_package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let database = fake_database(
            &result_package,
            vec![Response::Items(vec![solution("0x01")])],
            vec![Response::Items(vec![hydration(
                &result_package,
                "0x01",
                "Ada",
            )])],
        );
        let _failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(database.events.opens.load(Ordering::Acquire), 0);
        assert_eq!(database.events.queries.load(Ordering::Acquire), 0);

        // Rebuild a successful immutable result after the one-shot failure.
        let database = fake_database(
            &result_package,
            vec![Response::Items(vec![solution("0x01")])],
            vec![Response::Items(vec![hydration(
                &result_package,
                "0x01",
                "Ada",
            )])],
        );
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let _thing_failure = inject_failure(AllocationSite::QueryThingHandle, 0);
        let mut thing = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_slot_thing_at(
                    result,
                    RESULT_ROWS,
                    0,
                    0,
                    0,
                    &built.model,
                    MATCH_EXACT,
                    SELECTION_ONE,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(thing.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_result_row_slot_thing_at(
                    result,
                    RESULT_ROWS,
                    0,
                    0,
                    0,
                    &built.model,
                    MATCH_EXACT,
                    SELECTION_ONE,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_projected_thing_close(&mut thing) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(database.events.closes.load(Ordering::Acquire), 1);

        // Field-group value handles reserve only the outer ABI owner and are retryable.
        let value_package = package("queryvaluealloc");
        // SAFETY: fixture owns exact generated handles.
        let mut built =
            unsafe { BuiltQuery::open(&value_package, TERMINAL_REDUCE, ROWS_BOUNDED_MANY) };
        let person = TypeId::new(TypeKind::Entity, "person").expect("test person type");
        let department_token = projected_field_token(&value_package, &person, "department");
        let mut department = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_field_open(
                    built.binding,
                    &department_token,
                    &mut department,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let group = TypeBridgeQueryFieldReferenceV1 {
            struct_size: size_of::<TypeBridgeQueryFieldReferenceV1>() as u32,
            version: DESCRIPTOR_VERSION,
            field: department,
            expected_field: &department_token,
            reserved: [0; 4],
        };
        let reducer = TypeBridgeQueryReducerV1 {
            struct_size: size_of::<TypeBridgeQueryReducerV1>() as u32,
            version: DESCRIPTOR_VERSION,
            kind: REDUCER_COUNT,
            reserved0: 0,
            input: ptr::null(),
            expected_field: ptr::null(),
            reserved: [0; 4],
        };
        let mut descriptor = root_terminal_descriptor(&built, TERMINAL_REDUCE_FIELD);
        descriptor.group_fields = &group;
        descriptor.group_field_count = 1;
        descriptor.reducers = &reducer;
        descriptor.reducer_count = 1;
        unsafe { replace_terminal(&mut built, &descriptor) };
        let database = fake_database(
            &value_package,
            vec![Response::Items(vec![solution("0x01")])],
            vec![Response::Items(vec![rematch_bindings(vec![
                hydrated_thing_value(
                    &value_package,
                    0,
                    "person",
                    "0x01",
                    &[("identifier", "Ada"), ("department", "Engineering")],
                ),
            ])])],
        );
        result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_REDUCE_FIELD,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let _value_failure = inject_failure(AllocationSite::QueryValueHandle, 0);
        let mut value = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_field_at(
                    result,
                    RESULT_FIELD_REDUCTION,
                    0,
                    0,
                    &department_token,
                    &mut value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(value.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_result_reduction_group_field_at(
                    result,
                    RESULT_FIELD_REDUCTION,
                    0,
                    0,
                    &department_token,
                    &mut value,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_projected_value_close(&mut value) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_field_close(&mut department) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn provider_failure_close_precedence_and_borrowed_semantic_reuse_are_stable() {
        let failure_package = package("queryerrorclose");
        // SAFETY: fixture owns exact generated handles.
        let built =
            unsafe { BuiltQuery::open(&failure_package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let database = fake_database_with_close_failure(
            &failure_package,
            vec![Response::Error],
            Vec::new(),
            true,
        );
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_statement_failed"
        );
        let visible = unsafe { diagnostic_visible_text(diagnostics) };
        let visible = String::from_utf8(visible).expect("diagnostic views are UTF-8");
        assert!(!visible.contains("provider-secret-query-failure"));
        assert!(!visible.contains("provider-secret-close-failure"));
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(database.events.closes.load(Ordering::Acquire), 1);

        let reuse_package = package("queryborrowreuse");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&reuse_package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let database = fake_database(
            &reuse_package,
            vec![Response::Error, Response::Items(vec![])],
            Vec::new(),
        );
        let mut transaction = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    &*database.value,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut interruption_limits = default_limits();
        interruption_limits.timeout_milliseconds = 0;
        let deadline_result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_query_execute_v1(
                    transaction,
                    built.terminal,
                    TERMINAL_ROWS,
                    &interruption_limits,
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(deadline_result_failure);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let cancelled_result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_query_execute_v1(
                    transaction,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(cancelled_result_failure);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(database.events.queries.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_query_execute_v1(
                    transaction,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_statement_failed"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_query_execute_v1(
                    transaction,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(database.events.queries.load(Ordering::Acquire), 2);
        assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn predispatch_fences_reject_limits_kind_cancel_and_equivalent_package() {
        let package_handle = package("queryfence");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package_handle, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let database = fake_database(&package_handle, Vec::new(), Vec::new());
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();

        let mut limits = default_limits();
        limits.statements = 0;
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    &limits,
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "statement_count_limit"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(database.events.opens.load(Ordering::Acquire), 0);
        let excessive_limits = [
            {
                let mut value = default_limits();
                value.timeout_milliseconds = MAX_QUERY_TIMEOUT_MILLISECONDS + 1;
                value
            },
            {
                let mut value = default_limits();
                value.items = MAX_QUERY_ITEMS + 1;
                value
            },
            {
                let mut value = default_limits();
                value.bytes = MAX_QUERY_BYTES + 1;
                value
            },
            {
                let mut value = default_limits();
                value.graph_nodes = MAX_QUERY_GRAPH_NODES + 1;
                value
            },
            {
                let mut value = default_limits();
                value.attribute_values = MAX_QUERY_ATTRIBUTE_VALUES + 1;
                value
            },
            {
                let mut value = default_limits();
                value.collection_members = MAX_QUERY_COLLECTION_MEMBERS + 1;
                value
            },
            {
                let mut value = default_limits();
                value.role_players = MAX_QUERY_ROLE_PLAYERS + 1;
                value
            },
            {
                let mut value = default_limits();
                value.statements = MAX_QUERY_STATEMENTS + 1;
                value
            },
        ];
        for excessive in &excessive_limits {
            assert_eq!(
                parse_common_limits(Some(*excessive)).expect("layout remains canonical"),
                QueryExecutionResourceLimits::default(),
            );
            assert_eq!(database.events.opens.load(Ordering::Acquire), 0);
        }
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let cancelled_result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(cancelled_result_failure);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let equivalent = package("queryfence");
        let other = fake_database(&equivalent, Vec::new(), Vec::new());
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*other.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(database.events.opens.load(Ordering::Acquire), 0);
        assert_eq!(other.events.opens.load(Ordering::Acquire), 0);

        let accepted = fake_database(
            &package_handle,
            vec![Response::Items(Vec::new())],
            Vec::new(),
        );
        limits = default_limits();
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*accepted.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    &limits,
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(accepted.events.opens.load(Ordering::Acquire), 1);
        assert_eq!(accepted.events.closes.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn deadline_and_cooperative_cancellation_have_distinct_outer_categories() {
        let package = package("querycancel");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();

        let deadline_database = fake_database(&package, Vec::new(), Vec::new());
        let mut limits = default_limits();
        limits.timeout_milliseconds = 0;
        let deadline_result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*deadline_database.value,
                    built.terminal,
                    TERMINAL_ROWS,
                    &limits,
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        drop(deadline_result_failure);
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        assert_eq!(
            unsafe { diagnostic_category(diagnostics) },
            TypeBridgeExecutionDiagnosticCategory::ResourceLimit
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(deadline_database.events.opens.load(Ordering::Acquire), 0);
        assert_eq!(deadline_database.events.queries.load(Ordering::Acquire), 0);

        // One absolute deadline includes the bounded C alias/preflight walk;
        // expiry during that read-only phase still precedes nominal kind checks
        // and result-handle reservation.
        limits.timeout_milliseconds = 1;
        QUERY_PREFLIGHT_DELAY_MILLISECONDS.with(|delay| delay.set(5));
        let delayed_result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*deadline_database.value,
                    built.terminal,
                    TERMINAL_ROWS,
                    &limits,
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(delayed_result_failure);
        assert_eq!(deadline_database.events.opens.load(Ordering::Acquire), 0);

        let open_database = fake_database_with_pending_open(&package);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        let requester = request_cancellation_soon(cancellation);
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*open_database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert_eq!(
            requester.join().expect("cancellation requester joins"),
            TypeBridgeStatus::Ok
        );
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        assert_eq!(
            unsafe { diagnostic_category(diagnostics) },
            TypeBridgeExecutionDiagnosticCategory::Cancelled
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(open_database.events.opens.load(Ordering::Acquire), 1);
        assert_eq!(open_database.events.queries.load(Ordering::Acquire), 0);
        assert_eq!(open_database.events.closes.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );

        let statement_database = fake_database(&package, vec![Response::Pending], Vec::new());
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        let requester = request_cancellation_soon(cancellation);
        assert_eq!(
            unsafe {
                type_bridge_database_query_execute_v1(
                    &*statement_database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert_eq!(
            requester.join().expect("cancellation requester joins"),
            TypeBridgeStatus::Ok
        );
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        assert_eq!(
            unsafe { diagnostic_category(diagnostics) },
            TypeBridgeExecutionDiagnosticCategory::Cancelled
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(statement_database.events.opens.load(Ordering::Acquire), 1);
        assert_eq!(statement_database.events.queries.load(Ordering::Acquire), 1);
        assert_eq!(statement_database.events.closes.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn every_postdispatch_panic_closes_owned_and_poisons_borrowed() {
        for materializer in [false, true] {
            let package = package(if materializer {
                "querymatpanic"
            } else {
                "queryproviderpanic"
            });
            // SAFETY: fixture owns exact generated handles.
            let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
            let queries = if materializer {
                vec![Response::Items(vec![solution("0x01")])]
            } else {
                vec![Response::Panic]
            };
            let hydrations = if materializer {
                vec![Response::Items(vec![hydration(&package, "0x01", "Ada")])]
            } else {
                Vec::new()
            };
            let database = fake_database(&package, queries, hydrations);
            if materializer {
                MATERIALIZATION_PANIC.with(|armed| armed.set(true));
            }
            let mut result = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            let status = unsafe {
                type_bridge_database_query_execute_v1(
                    &*database.value,
                    built.terminal,
                    TERMINAL_FIRST,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            };
            let failure_code =
                (!diagnostics.is_null()).then(|| unsafe { diagnostic_code(diagnostics) });
            assert_eq!(
                status,
                TypeBridgeStatus::Panic,
                "materializer={materializer}: {failure_code:?}"
            );
            assert!(result.is_null());
            assert!(diagnostics.is_null());
            assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
        }

        for materializer in [false, true] {
            let package = package(if materializer {
                "queryborrowmat"
            } else {
                "queryborrowprovider"
            });
            // SAFETY: fixture owns exact generated handles.
            let built = unsafe { BuiltQuery::open(&package, TERMINAL_FIRST, ROWS_BOUNDED_MANY) };
            let queries = if materializer {
                vec![Response::Items(vec![solution("0x01")])]
            } else {
                vec![Response::Panic]
            };
            let hydrations = if materializer {
                vec![Response::Items(vec![hydration(&package, "0x01", "Ada")])]
            } else {
                Vec::new()
            };
            let database = fake_database(&package, queries, hydrations);
            let mut transaction = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_read_transaction_open(
                        &*database.value,
                        ptr::null(),
                        &mut transaction,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            if materializer {
                MATERIALIZATION_PANIC.with(|armed| armed.set(true));
            }
            let mut result = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_read_transaction_query_execute_v1(
                        transaction,
                        built.terminal,
                        TERMINAL_FIRST,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Panic,
            );
            let dispatched = database.events.queries.load(Ordering::Acquire);
            assert_eq!(
                unsafe {
                    type_bridge_read_transaction_query_execute_v1(
                        transaction,
                        built.terminal,
                        TERMINAL_FIRST,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed,
            );
            assert_eq!(database.events.queries.load(Ordering::Acquire), dispatched);
            unsafe { close_diagnostics(&mut diagnostics) };
            assert_eq!(
                unsafe { type_bridge_read_transaction_close(&mut transaction, &mut diagnostics) },
                TypeBridgeStatus::Ok,
            );
            assert!(diagnostics.is_null());
            assert_eq!(database.events.closes.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn remote_count_round_trip_retains_exact_lineage_and_one_shot_lifetimes() {
        let mut package_handle = Box::into_raw(Box::new(package("queryremote")));
        // SAFETY: package remains live through construction.
        let mut built =
            unsafe { BuiltQuery::open(&*package_handle, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let mut remote = remote_fixture(0x31);
        // SAFETY: package and advertisement remain immutable for context construction.
        let mut context = unsafe { open_remote_context(package_handle, &remote.advertisement) };

        let equivalent = package("queryremote");
        // SAFETY: equivalent fixture owns all constructed handles.
        let other = unsafe { BuiltQuery::open(&equivalent, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let mut rejected = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    other.terminal,
                    TERMINAL_COUNT,
                    ptr::null(),
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(rejected.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_package_lineage_mismatch"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let cancelled_pending_failure = inject_failure(AllocationSite::QueryRemotePendingHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    built.terminal,
                    TERMINAL_ROWS,
                    cancellation,
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert!(rejected.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(cancelled_pending_failure);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );

        // A valid FFI claim attempt consumes the pending authority on every
        // semantic outcome, including invocation-entry cancellation.
        let mut cancelled_pending =
            unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let mut cancelled_claim = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_claim(
                    cancelled_pending,
                    cancellation,
                    &mut cancelled_claim,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        assert!(cancelled_claim.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_claim(
                    cancelled_pending,
                    ptr::null(),
                    &mut cancelled_claim,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(cancelled_claim.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_remote_claim_consumed"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut cancelled_pending) },
            TypeBridgeStatus::Ok,
        );

        // Context preparation must use its copied advertisement.
        remote.advertisement.fill(b'x');
        // SAFETY: retained terminal and context share exact package lineage.
        let mut pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        // SAFETY: request bytes borrow the pending handle for this immediate copy.
        let request_view = unsafe { pending_request(pending) };
        let request =
            unsafe { std::slice::from_raw_parts(request_view.data, request_view.length).to_vec() };
        // Every preparation of the reusable terminal mints a fresh invocation
        // nonce/request proof, even when the query shape and limits are equal.
        let mut second_pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let second_request_view = unsafe { pending_request(second_pending) };
        let second_request = unsafe {
            std::slice::from_raw_parts(second_request_view.data, second_request_view.length)
                .to_vec()
        };
        let first_envelope = RemoteQueryRequestV2::decode(&request).expect("first request");
        let second_envelope =
            RemoteQueryRequestV2::decode(&second_request).expect("second request");
        assert_ne!(first_envelope.nonce(), second_envelope.nonce());
        assert_ne!(request, second_request);
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut second_pending) },
            TypeBridgeStatus::Ok,
        );

        unsafe { built.close_builders() };
        assert_eq!(
            unsafe { type_bridge_query_terminal_close(&mut built.terminal) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package_handle) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: pending independently owns the exact request after ancestors close.
        let retained_view = unsafe { pending_request(pending) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(retained_view.data, retained_view.length) },
            request
        );
        let mut response_limit = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_response_snapshot_limit(
                    pending,
                    &mut response_limit,
                )
            },
            TypeBridgeStatus::Ok,
        );

        // SAFETY: pending has one unclaimed reply slot.
        let mut claim = unsafe { claim_remote(pending) };
        let mut duplicate = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_claim(
                    pending,
                    ptr::null(),
                    &mut duplicate,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert!(duplicate.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );

        let mut response = remote_count_reply(&request, &remote, 7);
        assert!(response.len() <= response_limit);
        let response_view = TypeBridgeByteView {
            data: response.as_ptr(),
            length: response.len(),
        };
        let mut result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
        response.fill(b'x');
        let mut count = 0;
        assert_eq!(
            unsafe { type_bridge_query_result_count(result, &mut count, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(count, 7);
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_prepare_zero_deadline_precedes_kind_and_pending_reservation() {
        let package = package("queryremotezero");
        // SAFETY: fixture owns exact generated handles and copied advertisement.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x32);
        let mut limits = default_limits();
        limits.timeout_milliseconds = 0;
        let mut context =
            unsafe { open_remote_context_with_limits(&package, &remote.advertisement, &limits) };
        let mut pending = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        let pending_failure = inject_failure(AllocationSite::QueryRemotePendingHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    &mut pending,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(pending.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(pending_failure);

        // The same absolute preparation deadline is rechecked after the
        // bounded read-only context/terminal graph walk.
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
        limits.timeout_milliseconds = 1;
        context =
            unsafe { open_remote_context_with_limits(&package, &remote.advertisement, &limits) };
        QUERY_PREFLIGHT_DELAY_MILLISECONDS.with(|delay| delay.set(5));
        let delayed_pending_failure = inject_failure(AllocationSite::QueryRemotePendingHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    built.terminal,
                    TERMINAL_ROWS,
                    ptr::null(),
                    &mut pending,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        drop(delayed_pending_failure);
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_preflight_and_allocation_failures_are_read_only_and_retryable() {
        let package = package("queryremotehostile");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x42);
        let advertisement = TypeBridgeByteView {
            data: remote.advertisement.as_ptr(),
            length: remote.advertisement.len(),
        };
        let mut diagnostics = ptr::null_mut();

        let aliased_context = unsafe {
            remote
                .advertisement
                .as_ptr()
                .add(1)
                .cast_mut()
                .cast::<*mut TypeBridgeQueryRemoteContext>()
        };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_context_open_v1(
                    &package,
                    advertisement,
                    ptr::null(),
                    aliased_context,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());

        let _handle_failure = inject_failure(AllocationSite::QueryRemoteContextHandle, 0);
        let mut context = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_context_open_v1(
                    &package,
                    advertisement,
                    ptr::null(),
                    &mut context,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(context.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        let _advertisement_failure =
            inject_failure(AllocationSite::QueryRemoteAdvertisementBytes, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_remote_context_open_v1(
                    &package,
                    advertisement,
                    ptr::null(),
                    &mut context,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(context.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        // SAFETY: one-shot allocation failure left every input reusable.
        context = unsafe { open_remote_context(&package, &remote.advertisement) };

        let _pending_failure = inject_failure(AllocationSite::QueryRemotePendingHandle, 0);
        let mut pending = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    context,
                    built.terminal,
                    TERMINAL_COUNT,
                    ptr::null(),
                    &mut pending,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(pending.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        // SAFETY: failed handle reservation preceded remote pending publication.
        pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        // SAFETY: pending owns one stable request view.
        let request_view = unsafe { pending_request(pending) };
        let request =
            unsafe { std::slice::from_raw_parts(request_view.data, request_view.length).to_vec() };

        let aliased_view = unsafe {
            request_view
                .data
                .add(1)
                .cast_mut()
                .cast::<TypeBridgeByteView>()
        };
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_request_bytes(pending, aliased_view) },
            TypeBridgeStatus::InvalidArgument,
        );
        let retained = unsafe { pending_request(pending) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(retained.data, retained.length) },
            request
        );
        let aliased_limit = unsafe { request_view.data.add(1).cast_mut().cast::<usize>() };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_response_snapshot_limit(pending, aliased_limit)
            },
            TypeBridgeStatus::InvalidArgument,
        );
        let mut response_limit = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_response_snapshot_limit(
                    pending,
                    &mut response_limit,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(response_limit, REMOTE_RESPONSE_SNAPSHOT_LIMIT);
        let retained = unsafe { pending_request(pending) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(retained.data, retained.length) },
            request
        );

        let aliased_claim = unsafe {
            request_view
                .data
                .add(1)
                .cast_mut()
                .cast::<*mut TypeBridgeQueryRemoteClaim>()
        };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_claim(
                    pending,
                    ptr::null(),
                    aliased_claim,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());
        let _claim_failure = inject_failure(AllocationSite::QueryRemoteClaimHandle, 0);
        let mut claim = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_claim(
                    pending,
                    ptr::null(),
                    &mut claim,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(claim.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        // SAFETY: failed claim reservation preceded the atomic one-shot claim.
        claim = unsafe { claim_remote(pending) };

        let response = remote_count_reply(&request, &remote, 11);
        let response_view = TypeBridgeByteView {
            data: response.as_ptr(),
            length: response.len(),
        };
        let aliased_result = unsafe {
            response
                .as_ptr()
                .add(1)
                .cast_mut()
                .cast::<*mut TypeBridgeQueryResult>()
        };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    aliased_result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(diagnostics.is_null());

        let _response_failure = inject_failure(AllocationSite::QueryRemoteResponseBytes, 0);
        let mut result = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert!(result.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let mut value = 0;
        assert_eq!(
            unsafe { type_bridge_query_result_count(result, &mut value, &mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(value, 11);
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_rows_page_exists_and_reduction_share_direct_result_readers() {
        let package = package("queryremotematrix");
        let remote = remote_fixture(0x61);
        // SAFETY: package and advertisement remain immutable for context construction.
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let mut diagnostics = ptr::null_mut();

        for (terminal_kind, result_kind) in [
            (TERMINAL_FIRST, RESULT_ROWS),
            (TERMINAL_PAGE, RESULT_PAGE),
            (TERMINAL_EXISTS, RESULT_EXISTS),
            (TERMINAL_REDUCE, RESULT_REDUCTION),
        ] {
            // SAFETY: each fixture owns exact generated handles.
            let built = unsafe { BuiltQuery::open(&package, terminal_kind, ROWS_BOUNDED_MANY) };
            // SAFETY: context and terminal retain exact package lineage.
            let mut pending = unsafe { prepare_remote(context, built.terminal, terminal_kind) };
            // SAFETY: pending request remains live while copied and claimed.
            let request_view = unsafe { pending_request(pending) };
            let request = unsafe {
                std::slice::from_raw_parts(request_view.data, request_view.length).to_vec()
            };
            // SAFETY: pending has one unclaimed reply slot.
            let mut claim = unsafe { claim_remote(pending) };
            let response = remote_terminal_reply(&request, &remote);
            let mut result = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_query_remote_claim_decode_v1(
                        claim,
                        terminal_kind,
                        ptr::null(),
                        TypeBridgeByteView {
                            data: response.as_ptr(),
                            length: response.len(),
                        },
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok,
            );
            let mut observed_kind = 0;
            assert_eq!(
                unsafe { type_bridge_query_result_kind(result, &mut observed_kind) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(observed_kind, result_kind);
            match result_kind {
                RESULT_ROWS | RESULT_PAGE => {
                    let mut rows = usize::MAX;
                    assert_eq!(
                        unsafe {
                            type_bridge_query_result_row_count(
                                result,
                                result_kind,
                                &mut rows,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    assert_eq!(rows, 0);
                    if result_kind == RESULT_PAGE {
                        let mut metadata = TypeBridgeQueryPageMetadataV1 {
                            struct_size: 0,
                            version: 0,
                            offset: u64::MAX,
                            limit: u64::MAX,
                            has_total: 1,
                            reserved0: [1; 7],
                            total: u64::MAX,
                            reserved: [1; 4],
                        };
                        assert_eq!(
                            unsafe {
                                type_bridge_query_result_page_metadata_v1(
                                    result,
                                    &mut metadata,
                                    &mut diagnostics,
                                )
                            },
                            TypeBridgeStatus::Ok,
                        );
                        assert_eq!((metadata.offset, metadata.limit), (0, 1));
                        assert_eq!(metadata.has_total, 0);
                    }
                }
                RESULT_EXISTS => {
                    let mut exists = 0;
                    assert_eq!(
                        unsafe {
                            type_bridge_query_result_exists(result, &mut exists, &mut diagnostics)
                        },
                        TypeBridgeStatus::Ok,
                    );
                    assert_eq!(exists, 1);
                }
                RESULT_REDUCTION => {
                    let mut rows = 0;
                    assert_eq!(
                        unsafe {
                            type_bridge_query_result_reduction_row_count(
                                result,
                                RESULT_REDUCTION,
                                &mut rows,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    assert_eq!(rows, 1);
                    let mut count = 0;
                    assert_eq!(
                        unsafe {
                            type_bridge_query_result_reduction_value_count(
                                result,
                                RESULT_REDUCTION,
                                0,
                                0,
                                &mut count,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    assert_eq!(count, 13);
                }
                _ => unreachable!(),
            }
            assert_eq!(
                unsafe { type_bridge_query_result_close(&mut result) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_remote_claim_close(&mut claim) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_remote_pending_close(&mut pending) },
                TypeBridgeStatus::Ok,
            );
        }
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_decode_allocation_is_retryable_but_cancel_and_oversize_consume() {
        let package = package("queryremoteconsume");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x71);
        // SAFETY: package and advertisement remain immutable for context construction.
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let mut diagnostics = ptr::null_mut();

        // Result-owner reservation is boundary work and must not consume the claim.
        let mut pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let request_view = unsafe { pending_request(pending) };
        let request =
            unsafe { std::slice::from_raw_parts(request_view.data, request_view.length).to_vec() };
        let mut claim = unsafe { claim_remote(pending) };
        let response = remote_count_reply(&request, &remote, 17);
        let response_view = TypeBridgeByteView {
            data: response.as_ptr(),
            length: response.len(),
        };
        let mut result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_ROWS,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        assert!(result.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_nominal_contract_mismatch",
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        // A hostile cross-family cast is rejected before the claim is consumed,
        // so the exact count-family decoder remains usable.
        let _result_failure = inject_failure(AllocationSite::QueryResultHandle, 0);
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_result_close(&mut result) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );

        // Local cancellation is a semantic decode attempt and consumes the reply claim.
        pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        claim = unsafe { claim_remote(pending) };
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    cancellation,
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled,
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_remote_claim_consumed"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );

        // A complete over-ceiling caller snapshot is rejected before copying and consumes.
        pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
        let mut limit = 0;
        assert_eq!(
            unsafe {
                type_bridge_query_remote_pending_response_snapshot_limit(pending, &mut limit)
            },
            TypeBridgeStatus::Ok,
        );
        claim = unsafe { claim_remote(pending) };
        let oversized = vec![0x5a; limit + 1];
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    TypeBridgeByteView {
                        data: oversized.as_ptr(),
                        length: oversized.len(),
                    },
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_remote_response_limit_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                type_bridge_query_remote_claim_decode_v1(
                    claim,
                    TERMINAL_COUNT,
                    ptr::null(),
                    response_view,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed,
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_query_remote_claim_consumed"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_query_remote_claim_close(&mut claim) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_pending_close(&mut pending) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_authenticated_failure_matrix_is_atomic_redacted_and_consuming() {
        let package = package("queryremoteauthmatrix");
        // SAFETY: fixture owns exact generated handles.
        let built = unsafe { BuiltQuery::open(&package, TERMINAL_COUNT, ROWS_BOUNDED_MANY) };
        let remote = remote_fixture(0x79);
        let unsupported_contract = RemoteCapabilities::new(
            ContractCapabilitySet::new(),
            RemoteExecutorBinding::new("c-query-remote-unsupported", "epoch-00000000001")
                .expect("unsupported executor binding"),
            remote.signer.public_key(),
        );
        let unsupported_advertisement = unsupported_contract
            .encode()
            .expect("unsupported advertisement bytes");
        let mut unsupported_context =
            unsafe { open_remote_context(&package, &unsupported_advertisement) };
        let mut unsupported_pending = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_query_remote_prepare_v1(
                    unsupported_context,
                    built.terminal,
                    TERMINAL_COUNT,
                    ptr::null(),
                    &mut unsupported_pending,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Unsupported,
        );
        assert!(unsupported_pending.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "unsupported_required_capability"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut unsupported_context) },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: package and advertisement remain immutable for context construction.
        let mut context = unsafe { open_remote_context(&package, &remote.advertisement) };
        let cases = [
            (
                HostileRemoteReply::MalformedOuter,
                TypeBridgeStatus::InvalidArgument,
                "query_remote_reply_malformed",
            ),
            (
                HostileRemoteReply::BadSignature,
                TypeBridgeStatus::ExecutionFailed,
                "query_remote_signature_invalid",
            ),
            (
                HostileRemoteReply::WrongNonce,
                TypeBridgeStatus::ExecutionFailed,
                "query_remote_v2_nonce_mismatch",
            ),
            (
                HostileRemoteReply::WrongRequest,
                TypeBridgeStatus::ExecutionFailed,
                "query_remote_v2_request_mismatch",
            ),
            (
                HostileRemoteReply::ForeignSchemaOrProfilePlan,
                TypeBridgeStatus::ExecutionFailed,
                "query_remote_v2_plan_mismatch",
            ),
            (
                HostileRemoteReply::ForeignCapabilityAdvertisement,
                TypeBridgeStatus::ExecutionFailed,
                "query_remote_signature_invalid",
            ),
            (
                HostileRemoteReply::UnknownPayloadFormat,
                TypeBridgeStatus::InvalidArgument,
                "query_remote_v2_format_unsupported",
            ),
        ];

        for (kind, expected_status, expected_code) in cases {
            // Every case receives a fresh invocation token and one-shot claim.
            let mut pending = unsafe { prepare_remote(context, built.terminal, TERMINAL_COUNT) };
            let request_view = unsafe { pending_request(pending) };
            let request = unsafe {
                std::slice::from_raw_parts(request_view.data, request_view.length).to_vec()
            };
            let mut claim = unsafe { claim_remote(pending) };
            let response = hostile_remote_count_reply(&request, &remote, kind);
            let view = TypeBridgeByteView {
                data: response.as_ptr(),
                length: response.len(),
            };
            let mut result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
            let mut diagnostics = ptr::null_mut();
            assert_eq!(
                unsafe {
                    type_bridge_query_remote_claim_decode_v1(
                        claim,
                        TERMINAL_COUNT,
                        ptr::null(),
                        view,
                        &mut result,
                        &mut diagnostics,
                    )
                },
                expected_status,
                "{kind:?}",
            );
            assert!(result.is_null(), "{kind:?}");
            assert_eq!(
                unsafe { diagnostic_code(diagnostics) },
                expected_code,
                "{kind:?}"
            );
            let diagnostic_evidence = unsafe { diagnostic_visible_text(diagnostics) };
            assert!(
                !diagnostic_evidence
                    .windows(b"secret-provider-reply".len())
                    .any(|window| window == b"secret-provider-reply"),
                "{kind:?} copied unauthenticated response text",
            );
            unsafe { close_diagnostics(&mut diagnostics) };

            // Authentication/correlation/malformed outcomes are semantic
            // attempts: the exact claim cannot be retried or replayed.
            result = std::ptr::NonNull::<TypeBridgeQueryResult>::dangling().as_ptr();
            assert_eq!(
                unsafe {
                    type_bridge_query_remote_claim_decode_v1(
                        claim,
                        TERMINAL_COUNT,
                        ptr::null(),
                        view,
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed,
                "{kind:?}",
            );
            assert!(result.is_null());
            assert_eq!(
                unsafe { diagnostic_code(diagnostics) },
                "c_query_remote_claim_consumed",
                "{kind:?}",
            );
            unsafe { close_diagnostics(&mut diagnostics) };
            assert_eq!(
                unsafe { type_bridge_query_remote_claim_close(&mut claim) },
                TypeBridgeStatus::Ok,
            );
            assert_eq!(
                unsafe { type_bridge_query_remote_pending_close(&mut pending) },
                TypeBridgeStatus::Ok,
            );
        }
        assert_eq!(
            unsafe { type_bridge_query_remote_context_close(&mut context) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn remote_structured_failure_is_redacted_and_semantically_consumes_claim() {
        let observation = observe_workforce_v2_remote_structured_diagnostic();
        assert_eq!(observation["code"], "remote_application_failure");
        assert_eq!(observation["redacted"], true);
        assert_eq!(observation["claim_consumed"], true);
    }

    #[test]
    fn workforce_v2_c_deterministic_proof_fragment() {
        let results = vec![
            serde_json::json!({
                "observation_ref": "cancellation_direct",
                "proof_kind": "direct_runtime",
                "test_id": "query::tests::workforce_v2_c_deterministic_proof_fragment",
                "outcome": "passed",
                "observation": observe_workforce_v2_direct_cancellation(),
            }),
            serde_json::json!({
                "observation_ref": "cancellation_remote",
                "proof_kind": "remote_runtime",
                "test_id": "query::tests::workforce_v2_c_deterministic_proof_fragment",
                "outcome": "passed",
                "observation": observe_workforce_v2_remote_cancellation(),
            }),
            serde_json::json!({
                "observation_ref": "remote_structured_diagnostic",
                "proof_kind": "diagnostic",
                "test_id": "query::tests::workforce_v2_c_deterministic_proof_fragment",
                "outcome": "passed",
                "observation": observe_workforce_v2_remote_structured_diagnostic(),
            }),
        ];
        if let Some((destination, run_nonce)) = requested_workforce_v2_proof() {
            publish_workforce_v2_proof(&destination, &run_nonce, results);
        }
    }
}
