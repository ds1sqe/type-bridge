use std::ptr;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticPathSegment,
    SdkExecutionDiagnostic, SdkQueryDiagnosticPathKind,
};

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus, close_box, guarded};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};

/// Stable C category for one SDK execution diagnostic.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeExecutionDiagnosticCategory {
    /// Caller input violates the generated SDK contract.
    InvalidInput = 1,
    /// The runtime or provider lacks a required capability.
    UnsupportedCapability = 2,
    /// A stable resource ceiling was exceeded.
    ResourceLimit = 3,
    /// Schema, projection, request, or result identity disagreed.
    Integrity = 4,
    /// The provider failed an operation.
    Provider = 5,
    /// Transaction lifecycle or commit certainty failed.
    Transaction = 6,
    /// The operation was cancelled cooperatively before or during supported work.
    Cancelled = 7,
    /// TypeBridge failed internally.
    Internal = 8,
}

/// Stable C kind for one typed execution-diagnostic path segment.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeExecutionDiagnosticPathKind {
    /// A stable SDK argument name in `primary`.
    Argument = 1,
    /// A zero-based collection position in `index`.
    Index = 2,
    /// A type kind and label in `primary` and `secondary`.
    Type = 3,
    /// Owner kind, owner label, and attribute label in the first three text views.
    Field = 4,
    /// Declaring relation and role label in `primary` and `secondary`.
    Role = 5,
    /// The complete typed-query request envelope.
    QueryRequest = 6,
    /// The typed-query graph plan.
    QueryPlan = 7,
    /// The selected typed-query terminal operation.
    QueryOperation = 8,
    /// The typed-query predicate tree.
    QueryPredicate = 9,
    /// The typed-query output shape.
    QueryOutput = 10,
    /// Provider solution or hydration evidence.
    QueryProviderEvidence = 11,
    /// The validated typed-query result envelope.
    QueryResult = 12,
    /// A plan-local binding ordinal in `index`.
    QueryBinding = 13,
    /// A descriptor and field identity in `primary` and `secondary`.
    QueryField = 14,
    /// A descriptor and role identity in `primary` and `secondary`.
    QueryRole = 15,
    /// A plan-local role-edge ordinal in `index`.
    QueryRoleEdge = 16,
    /// A zero-based output slot in `index`.
    QueryOutputSlot = 17,
    /// A declared output member in `primary`.
    QueryOutputName = 18,
    /// A canonical remote-contract field identity in `primary`.
    ContractField = 19,
    /// A canonical remote-contract identifier in `primary`.
    ContractIdentity = 20,
    /// A newer path kind unknown to this ABI minor.
    Unknown = 255,
}

/// Stable C kind for one typed execution-diagnostic detail.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeExecutionDiagnosticDetailKind {
    /// A Boolean in `boolean_value`.
    Boolean = 1,
    /// A non-negative count in `unsigned_value`.
    Count = 2,
    /// A non-negative byte count in `unsigned_value`.
    ByteCount = 3,
    /// A capability identity in `primary`.
    Capability = 4,
    /// A canonical scalar-domain spelling in `primary`.
    ValueType = 5,
    /// A type kind and label in `primary` and `secondary`.
    Type = 6,
    /// Owner kind, owner label, and attribute label in the first three text views.
    Field = 7,
    /// Declaring relation and role label in `primary` and `secondary`.
    Role = 8,
    /// Fingerprint domain, canonicalization, profile, and digest in all four text views.
    Fingerprint = 9,
    /// A redacted provider-operation spelling in `primary`.
    ProviderOperation = 10,
    /// A classified commit-outcome spelling in `primary`.
    CommitOutcome = 11,
    /// A closed query category or bounded redacted query identity in `primary`.
    Text = 12,
    /// A signed integer available through the signed-detail accessor.
    Signed = 13,
    /// A bounded ordered identity list available through list accessors.
    TextList = 14,
    /// A newer detail kind unknown to this ABI minor.
    Unknown = 255,
}

/// Borrowed summary of one execution diagnostic.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeExecutionDiagnosticViewV1 {
    /// Exact size of this view.
    pub struct_size: u32,
    /// In-memory diagnostic contract version.
    pub version: u32,
    /// Stable category.
    pub category: TypeBridgeExecutionDiagnosticCategory,
    /// Reserved zero word.
    pub reserved0: u32,
    /// Stable machine-readable code.
    pub code: TypeBridgeByteView,
    /// Stable redacted message.
    pub message: TypeBridgeByteView,
    /// Number of typed path segments.
    pub path_count: usize,
    /// Number of deterministic typed details.
    pub detail_count: usize,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Borrowed view of one typed execution-diagnostic path segment.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeExecutionDiagnosticPathViewV1 {
    /// Exact size of this view.
    pub struct_size: u32,
    /// Stable segment kind.
    pub kind: TypeBridgeExecutionDiagnosticPathKind,
    /// Zero-based position for an index segment, otherwise zero.
    pub index: u64,
    /// First kind-specific text component.
    pub primary: TypeBridgeByteView,
    /// Second kind-specific text component.
    pub secondary: TypeBridgeByteView,
    /// Third kind-specific text component.
    pub tertiary: TypeBridgeByteView,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Borrowed view of one typed execution-diagnostic detail.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeExecutionDiagnosticDetailViewV1 {
    /// Exact size of this view.
    pub struct_size: u32,
    /// Stable detail-value kind.
    pub kind: TypeBridgeExecutionDiagnosticDetailKind,
    /// Exact Boolean value (zero or one), otherwise zero.
    pub boolean_value: u8,
    /// Reserved zero padding.
    pub reserved0: [u8; 7],
    /// Count or byte count, otherwise zero.
    pub unsigned_value: u64,
    /// Stable detail key.
    pub key: TypeBridgeByteView,
    /// First kind-specific text component.
    pub primary: TypeBridgeByteView,
    /// Second kind-specific text component.
    pub secondary: TypeBridgeByteView,
    /// Third kind-specific text component.
    pub tertiary: TypeBridgeByteView,
    /// Fourth kind-specific text component.
    pub quaternary: TypeBridgeByteView,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

#[derive(Debug)]
struct OwnedPath {
    kind: TypeBridgeExecutionDiagnosticPathKind,
    index: u64,
    text: [Vec<u8>; 3],
}

#[derive(Debug)]
struct OwnedDetail {
    key: Vec<u8>,
    kind: TypeBridgeExecutionDiagnosticDetailKind,
    boolean_value: bool,
    unsigned_value: u64,
    signed_value: i64,
    text: [Vec<u8>; 4],
    list: Vec<Vec<u8>>,
}

#[derive(Debug)]
struct OwnedDiagnostic {
    version: u32,
    category: TypeBridgeExecutionDiagnosticCategory,
    code: Vec<u8>,
    message: Vec<u8>,
    path: Vec<OwnedPath>,
    details: Vec<OwnedDetail>,
}

/// Opaque ordered SDK execution-diagnostic collection owned by Rust.
pub struct TypeBridgeExecutionDiagnostics {
    items: Vec<OwnedDiagnostic>,
}

fn check_slice<T>(values: &[T], preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
    let bytes = values
        .len()
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(TypeBridgeStatus::ResourceLimit)?;
    preflight.check_bytes(values.as_ptr().cast(), bytes)
}

impl TypeBridgeExecutionDiagnostics {
    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        check_slice(&self.items, preflight)?;
        for diagnostic in &self.items {
            for value in [&diagnostic.code, &diagnostic.message] {
                check_slice(value, preflight)?;
            }
            check_slice(&diagnostic.path, preflight)?;
            for path in &diagnostic.path {
                for value in &path.text {
                    check_slice(value, preflight)?;
                }
            }
            check_slice(&diagnostic.details, preflight)?;
            for detail in &diagnostic.details {
                check_slice(&detail.key, preflight)?;
                for value in &detail.text {
                    check_slice(value, preflight)?;
                }
                check_slice(&detail.list, preflight)?;
                for value in &detail.list {
                    check_slice(value, preflight)?;
                }
            }
        }
        Ok(())
    }
}

unsafe fn preflight_diagnostics_output<T>(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    output: *mut T,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    let preflight = direct_output_preflight(&[(output.cast(), std::mem::size_of::<T>())])?;
    if !diagnostics.is_null() {
        preflight.check_bytes(
            diagnostics.cast(),
            std::mem::size_of::<TypeBridgeExecutionDiagnostics>(),
        )?;
        // SAFETY: the complete immutable outer handle was checked first.
        unsafe { &*diagnostics }.check_borrowed_ranges(&preflight)?;
    }
    Ok(preflight)
}

fn bytes(value: impl AsRef<str>) -> Vec<u8> {
    value.as_ref().as_bytes().to_vec()
}

fn type_parts(value: &type_bridge_contract::id::TypeId) -> [Vec<u8>; 2] {
    [
        bytes(match value.kind() {
            type_bridge_contract::id::TypeKind::Entity => "entity",
            type_bridge_contract::id::TypeKind::Relation => "relation",
            type_bridge_contract::id::TypeKind::Attribute => "attribute",
            type_bridge_contract::id::TypeKind::Struct => "struct",
        }),
        bytes(value.label().as_str()),
    ]
}

impl From<&SdkDiagnosticPathSegment> for OwnedPath {
    fn from(value: &SdkDiagnosticPathSegment) -> Self {
        let mut output = Self {
            kind: TypeBridgeExecutionDiagnosticPathKind::Unknown,
            index: 0,
            text: std::array::from_fn(|_| Vec::new()),
        };
        match value {
            SdkDiagnosticPathSegment::Argument(name) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::Argument;
                output.text[0] = bytes(name.as_str());
            }
            SdkDiagnosticPathSegment::Index(index) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::Index;
                output.index = *index;
            }
            SdkDiagnosticPathSegment::Type(id) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::Type;
                let parts = type_parts(id);
                output.text[0] = parts[0].clone();
                output.text[1] = parts[1].clone();
            }
            SdkDiagnosticPathSegment::Field(id) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::Field;
                let parts = type_parts(id.owner());
                output.text[0] = parts[0].clone();
                output.text[1] = parts[1].clone();
                output.text[2] = bytes(id.attribute().label().as_str());
            }
            SdkDiagnosticPathSegment::Role(id) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::Role;
                output.text[0] = bytes(id.declaring_relation().as_str());
                output.text[1] = bytes(id.label().as_str());
            }
            SdkDiagnosticPathSegment::Query(kind) => {
                output.kind = match kind {
                    SdkQueryDiagnosticPathKind::Request => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryRequest
                    }
                    SdkQueryDiagnosticPathKind::Plan => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryPlan
                    }
                    SdkQueryDiagnosticPathKind::Operation => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryOperation
                    }
                    SdkQueryDiagnosticPathKind::Predicate => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryPredicate
                    }
                    SdkQueryDiagnosticPathKind::Output => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryOutput
                    }
                    SdkQueryDiagnosticPathKind::ProviderEvidence => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryProviderEvidence
                    }
                    SdkQueryDiagnosticPathKind::Result => {
                        TypeBridgeExecutionDiagnosticPathKind::QueryResult
                    }
                    _ => TypeBridgeExecutionDiagnosticPathKind::Unknown,
                };
            }
            SdkDiagnosticPathSegment::QueryBinding(index) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryBinding;
                output.index = u64::from(*index);
            }
            SdkDiagnosticPathSegment::QueryField { owner, name } => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryField;
                output.text[0] = bytes(owner.as_str());
                output.text[1] = bytes(name.as_str());
            }
            SdkDiagnosticPathSegment::QueryRole { owner, name } => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryRole;
                output.text[0] = bytes(owner.as_str());
                output.text[1] = bytes(name.as_str());
            }
            SdkDiagnosticPathSegment::QueryRoleEdge(index) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryRoleEdge;
                output.index = u64::from(*index);
            }
            SdkDiagnosticPathSegment::QueryOutputSlot(index) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryOutputSlot;
                output.index = *index;
            }
            SdkDiagnosticPathSegment::QueryOutputName(name) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::QueryOutputName;
                output.text[0] = bytes(name.as_str());
            }
            SdkDiagnosticPathSegment::ContractField(name) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::ContractField;
                output.text[0] = bytes(name.as_str());
            }
            SdkDiagnosticPathSegment::ContractIdentity(name) => {
                output.kind = TypeBridgeExecutionDiagnosticPathKind::ContractIdentity;
                output.text[0] = bytes(name.as_str());
            }
            _ => {}
        }
        output
    }
}

impl OwnedDetail {
    fn from_sdk(
        key: &type_bridge_contract::sdk_diagnostic::SdkDiagnosticName,
        value: &SdkDiagnosticDetailValue,
    ) -> Self {
        let mut output = Self {
            key: bytes(key.as_str()),
            kind: TypeBridgeExecutionDiagnosticDetailKind::Unknown,
            boolean_value: false,
            unsigned_value: 0,
            signed_value: 0,
            text: std::array::from_fn(|_| Vec::new()),
            list: Vec::new(),
        };
        match value {
            SdkDiagnosticDetailValue::Boolean(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Boolean;
                output.boolean_value = *value;
            }
            SdkDiagnosticDetailValue::Count(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Count;
                output.unsigned_value = *value;
            }
            SdkDiagnosticDetailValue::ByteCount(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::ByteCount;
                output.unsigned_value = *value;
            }
            SdkDiagnosticDetailValue::Capability(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Capability;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::ValueType(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::ValueType;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::Type(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Type;
                let parts = type_parts(value);
                output.text[0] = parts[0].clone();
                output.text[1] = parts[1].clone();
            }
            SdkDiagnosticDetailValue::Field(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Field;
                let parts = type_parts(value.owner());
                output.text[0] = parts[0].clone();
                output.text[1] = parts[1].clone();
                output.text[2] = bytes(value.attribute().label().as_str());
            }
            SdkDiagnosticDetailValue::Role(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Role;
                output.text[0] = bytes(value.declaring_relation().as_str());
                output.text[1] = bytes(value.label().as_str());
            }
            SdkDiagnosticDetailValue::Fingerprint(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Fingerprint;
                output.text[0] = bytes(value.domain().as_str());
                output.text[1] = bytes(value.canonicalization().as_str());
                output.text[2] = value
                    .semantic_profile()
                    .map_or_else(Vec::new, |profile| bytes(profile.as_str()));
                output.text[3] = bytes(value.digest().to_hex());
            }
            SdkDiagnosticDetailValue::ProviderOperation(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::ProviderOperation;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::CommitOutcome(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::CommitOutcome;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::Signed(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Signed;
                output.signed_value = *value;
            }
            SdkDiagnosticDetailValue::QueryCategory(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Text;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::QueryIdentity(value) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::Text;
                output.text[0] = bytes(value.as_str());
            }
            SdkDiagnosticDetailValue::QueryIdentityList(values) => {
                output.kind = TypeBridgeExecutionDiagnosticDetailKind::TextList;
                output.list = values.iter().map(|value| bytes(value.as_str())).collect();
            }
            _ => {}
        }
        output
    }
}

impl From<SdkExecutionDiagnostic> for OwnedDiagnostic {
    fn from(value: SdkExecutionDiagnostic) -> Self {
        let category = match value.category() {
            SdkDiagnosticCategory::InvalidInput => {
                TypeBridgeExecutionDiagnosticCategory::InvalidInput
            }
            SdkDiagnosticCategory::UnsupportedCapability => {
                TypeBridgeExecutionDiagnosticCategory::UnsupportedCapability
            }
            SdkDiagnosticCategory::ResourceLimit => {
                TypeBridgeExecutionDiagnosticCategory::ResourceLimit
            }
            SdkDiagnosticCategory::Integrity => TypeBridgeExecutionDiagnosticCategory::Integrity,
            SdkDiagnosticCategory::Provider => TypeBridgeExecutionDiagnosticCategory::Provider,
            SdkDiagnosticCategory::Transaction => {
                TypeBridgeExecutionDiagnosticCategory::Transaction
            }
            SdkDiagnosticCategory::Cancelled => TypeBridgeExecutionDiagnosticCategory::Cancelled,
            SdkDiagnosticCategory::Internal => TypeBridgeExecutionDiagnosticCategory::Internal,
            _ => TypeBridgeExecutionDiagnosticCategory::Internal,
        };
        Self {
            version: u32::from(value.version().get()),
            category,
            code: bytes(value.code().as_str()),
            message: bytes(value.message().as_str()),
            path: value.path().iter().map(OwnedPath::from).collect(),
            details: value
                .details()
                .iter()
                .map(|(key, detail)| OwnedDetail::from_sdk(key, detail))
                .collect(),
        }
    }
}

pub(crate) fn execution_diagnostics_handle(
    diagnostics: Vec<SdkExecutionDiagnostic>,
) -> TypeBridgeExecutionDiagnostics {
    TypeBridgeExecutionDiagnostics {
        items: diagnostics.into_iter().map(OwnedDiagnostic::from).collect(),
    }
}

pub(crate) fn status_for(diagnostic: &SdkExecutionDiagnostic) -> TypeBridgeStatus {
    if diagnostic.code().as_str() == "commit_outcome_unknown" {
        return TypeBridgeStatus::CommitOutcomeUnknown;
    }
    match diagnostic.category() {
        SdkDiagnosticCategory::ResourceLimit => TypeBridgeStatus::ResourceLimit,
        SdkDiagnosticCategory::Cancelled => TypeBridgeStatus::Cancelled,
        SdkDiagnosticCategory::InvalidInput => TypeBridgeStatus::InvalidArgument,
        SdkDiagnosticCategory::UnsupportedCapability => TypeBridgeStatus::Unsupported,
        SdkDiagnosticCategory::Integrity
        | SdkDiagnosticCategory::Provider
        | SdkDiagnosticCategory::Transaction
        | SdkDiagnosticCategory::Internal => TypeBridgeStatus::ExecutionFailed,
        _ => TypeBridgeStatus::ExecutionFailed,
    }
}

fn empty_view() -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    }
}

fn empty_diagnostic_view() -> TypeBridgeExecutionDiagnosticViewV1 {
    TypeBridgeExecutionDiagnosticViewV1 {
        struct_size: 0,
        version: 0,
        category: TypeBridgeExecutionDiagnosticCategory::Internal,
        reserved0: 0,
        code: empty_view(),
        message: empty_view(),
        path_count: 0,
        detail_count: 0,
        reserved: [0; 4],
    }
}

fn empty_path_view() -> TypeBridgeExecutionDiagnosticPathViewV1 {
    TypeBridgeExecutionDiagnosticPathViewV1 {
        struct_size: 0,
        kind: TypeBridgeExecutionDiagnosticPathKind::Unknown,
        index: 0,
        primary: empty_view(),
        secondary: empty_view(),
        tertiary: empty_view(),
        reserved: [0; 4],
    }
}

fn empty_detail_view() -> TypeBridgeExecutionDiagnosticDetailViewV1 {
    TypeBridgeExecutionDiagnosticDetailViewV1 {
        struct_size: 0,
        kind: TypeBridgeExecutionDiagnosticDetailKind::Unknown,
        boolean_value: 0,
        reserved0: [0; 7],
        unsigned_value: 0,
        key: empty_view(),
        primary: empty_view(),
        secondary: empty_view(),
        tertiary: empty_view(),
        quaternary: empty_view(),
        reserved: [0; 4],
    }
}

fn view(value: &[u8]) -> TypeBridgeByteView {
    if value.is_empty() {
        empty_view()
    } else {
        TypeBridgeByteView {
            data: value.as_ptr(),
            length: value.len(),
        }
    }
}

/// Return the number of ordered SDK execution diagnostics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_count(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_count) } {
        return status;
    }
    guarded(|| {
        if out_count.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller supplied a writable output slot.
        unsafe { out_count.write(0) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        // SAFETY: output was validated above.
        unsafe { out_count.write(diagnostics.items.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow one ordered SDK execution-diagnostic summary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_get_v1(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    index: usize,
    out_diagnostic: *mut TypeBridgeExecutionDiagnosticViewV1,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_diagnostic) } {
        return status;
    }
    guarded(|| {
        if out_diagnostic.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable output view.
        unsafe { out_diagnostic.write(empty_diagnostic_view()) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(item) = diagnostics.items.get(index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let struct_size = match u32::try_from(size_of::<TypeBridgeExecutionDiagnosticViewV1>()) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::Panic,
        };
        // SAFETY: output was validated and initialized above.
        unsafe {
            out_diagnostic.write(TypeBridgeExecutionDiagnosticViewV1 {
                struct_size,
                version: item.version,
                category: item.category,
                reserved0: 0,
                code: view(&item.code),
                message: view(&item.message),
                path_count: item.path.len(),
                detail_count: item.details.len(),
                reserved: [0; 4],
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Borrow one typed path segment from an SDK execution diagnostic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_path_get_v1(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    diagnostic_index: usize,
    path_index: usize,
    out_path: *mut TypeBridgeExecutionDiagnosticPathViewV1,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_path) } {
        return status;
    }
    guarded(|| {
        if out_path.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable output view.
        unsafe { out_path.write(empty_path_view()) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(path) = diagnostics
            .items
            .get(diagnostic_index)
            .and_then(|diagnostic| diagnostic.path.get(path_index))
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let struct_size = match u32::try_from(size_of::<TypeBridgeExecutionDiagnosticPathViewV1>())
        {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::Panic,
        };
        // SAFETY: output was validated and initialized above.
        unsafe {
            out_path.write(TypeBridgeExecutionDiagnosticPathViewV1 {
                struct_size,
                kind: path.kind,
                index: path.index,
                primary: view(&path.text[0]),
                secondary: view(&path.text[1]),
                tertiary: view(&path.text[2]),
                reserved: [0; 4],
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Borrow one deterministic typed detail from an SDK execution diagnostic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_detail_get_v1(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    diagnostic_index: usize,
    detail_index: usize,
    out_detail: *mut TypeBridgeExecutionDiagnosticDetailViewV1,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_detail) } {
        return status;
    }
    guarded(|| {
        if out_detail.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied one writable output view.
        unsafe { out_detail.write(empty_detail_view()) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(detail) = diagnostics
            .items
            .get(diagnostic_index)
            .and_then(|diagnostic| diagnostic.details.get(detail_index))
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let struct_size = match u32::try_from(size_of::<TypeBridgeExecutionDiagnosticDetailViewV1>())
        {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::Panic,
        };
        // SAFETY: output was validated and initialized above.
        unsafe {
            out_detail.write(TypeBridgeExecutionDiagnosticDetailViewV1 {
                struct_size,
                kind: detail.kind,
                boolean_value: u8::from(detail.boolean_value),
                reserved0: [0; 7],
                unsigned_value: detail.unsigned_value,
                key: view(&detail.key),
                primary: view(&detail.text[0]),
                secondary: view(&detail.text[1]),
                tertiary: view(&detail.text[2]),
                quaternary: view(&detail.text[3]),
                reserved: [0; 4],
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Read one signed detail without changing the frozen version-1 detail view.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_detail_signed(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    diagnostic_index: usize,
    detail_index: usize,
    out_value: *mut i64,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_value) } {
        return status;
    }
    guarded(|| {
        if out_value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller supplied a writable output slot.
        unsafe { out_value.write(0) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(detail) = diagnostics
            .items
            .get(diagnostic_index)
            .and_then(|diagnostic| diagnostic.details.get(detail_index))
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if detail.kind != TypeBridgeExecutionDiagnosticDetailKind::Signed {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the output was initialized above.
        unsafe { out_value.write(detail.signed_value) };
        TypeBridgeStatus::Ok
    })
}

/// Return the entry count for one bounded text-list detail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_detail_list_count(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    diagnostic_index: usize,
    detail_index: usize,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_count) } {
        return status;
    }
    guarded(|| {
        if out_count.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller supplied a writable output slot.
        unsafe { out_count.write(0) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(detail) = diagnostics
            .items
            .get(diagnostic_index)
            .and_then(|diagnostic| diagnostic.details.get(detail_index))
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if detail.kind != TypeBridgeExecutionDiagnosticDetailKind::TextList {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the output was initialized above.
        unsafe { out_count.write(detail.list.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow one entry from a bounded text-list detail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_detail_list_get(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
    diagnostic_index: usize,
    detail_index: usize,
    list_index: usize,
    out_value: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: complete diagnostics graph is fenced before any caller output write.
    if let Err(status) = unsafe { preflight_diagnostics_output(diagnostics, out_value) } {
        return status;
    }
    guarded(|| {
        if out_value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller supplied a writable output view.
        unsafe { out_value.write(empty_view()) };
        if diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable handle during this call.
        let diagnostics = unsafe { &*diagnostics };
        let Some(detail) = diagnostics
            .items
            .get(diagnostic_index)
            .and_then(|diagnostic| diagnostic.details.get(detail_index))
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if detail.kind != TypeBridgeExecutionDiagnosticDetailKind::TextList {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(item) = detail.list.get(list_index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        // SAFETY: the output was initialized above and borrows the live handle.
        unsafe { out_value.write(view(item)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one Rust-owned SDK execution-diagnostic collection and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_execution_diagnostics_close(
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(diagnostics) }
}

pub(crate) unsafe fn initialize_execution_outputs<T>(
    out_value: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    let output_size = std::mem::size_of::<*mut T>();
    let outputs_overlap = !out_value.is_null()
        && !out_diagnostics.is_null()
        && out_value
            .addr()
            .checked_add(output_size)
            .zip(out_diagnostics.addr().checked_add(output_size))
            .is_none_or(|(value_end, diagnostics_end)| {
                out_value.addr() < diagnostics_end && out_diagnostics.addr() < value_end
            });
    if outputs_overlap {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if !out_value.is_null() {
        // SAFETY: a non-null output slot is writable by the C caller contract.
        unsafe { out_value.write_unaligned(ptr::null_mut()) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: a non-null output slot is writable by the C caller contract.
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_value.is_null() || out_diagnostics.is_null() {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

pub(crate) fn return_execution_error(
    diagnostic: SdkExecutionDiagnostic,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let status = status_for(&diagnostic);
    // SAFETY: the shared output initializer proved this slot writable and distinct.
    unsafe {
        out_diagnostics.write_unaligned(Box::into_raw(Box::new(execution_diagnostics_handle(
            vec![diagnostic],
        ))))
    };
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> *mut TypeBridgeExecutionDiagnostics {
        Box::into_raw(Box::new(TypeBridgeExecutionDiagnostics {
            items: vec![OwnedDiagnostic {
                version: 1,
                category: TypeBridgeExecutionDiagnosticCategory::InvalidInput,
                code: b"query_diagnostic_code_with_alias_storage".to_vec(),
                message: b"Stable redacted query diagnostic message with ample alias storage"
                    .to_vec(),
                path: vec![OwnedPath {
                    kind: TypeBridgeExecutionDiagnosticPathKind::QueryOutputName,
                    index: 0,
                    text: [
                        b"output_member_with_ample_alias_storage".to_vec(),
                        Vec::new(),
                        Vec::new(),
                    ],
                }],
                details: vec![
                    OwnedDetail {
                        key: b"signed_bound_with_alias_storage".to_vec(),
                        kind: TypeBridgeExecutionDiagnosticDetailKind::Signed,
                        boolean_value: false,
                        unsigned_value: 0,
                        signed_value: -17,
                        text: std::array::from_fn(|_| Vec::new()),
                        list: Vec::new(),
                    },
                    OwnedDetail {
                        key: b"binding_list_with_alias_storage".to_vec(),
                        kind: TypeBridgeExecutionDiagnosticDetailKind::TextList,
                        boolean_value: false,
                        unsigned_value: 0,
                        signed_value: 0,
                        text: std::array::from_fn(|_| Vec::new()),
                        list: vec![
                            b"binding_identity_with_ample_alias_storage".to_vec(),
                            b"second_binding_identity".to_vec(),
                        ],
                    },
                ],
            }],
        }))
    }

    #[test]
    fn diagnostic_readers_preflight_the_complete_borrowed_graph_before_writes() {
        let mut diagnostics = fixture();

        // An output whose start is inside the opaque owner is rejected even
        // when its tail extends beyond the owner allocation.
        let outer_alias = unsafe { diagnostics.cast::<u8>().add(1).cast::<usize>() };
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_count(diagnostics, outer_alias) },
            TypeBridgeStatus::InvalidArgument,
        );

        let mut count = usize::MAX;
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_count(diagnostics, &mut count) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(count, 1);

        let mut diagnostic = empty_diagnostic_view();
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &mut diagnostic) },
            TypeBridgeStatus::Ok,
        );
        let code_alias = unsafe {
            diagnostic
                .code
                .data
                .add(1)
                .cast_mut()
                .cast::<TypeBridgeExecutionDiagnosticViewV1>()
        };
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, code_alias) },
            TypeBridgeStatus::InvalidArgument,
        );

        let message_tail_alias = unsafe {
            diagnostic
                .message
                .data
                .add(diagnostic.message.length - 1)
                .cast_mut()
                .cast::<TypeBridgeExecutionDiagnosticPathViewV1>()
        };
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0, 0, message_tail_alias)
            },
            TypeBridgeStatus::InvalidArgument,
        );

        let mut path = empty_path_view();
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0, 0, &mut path) },
            TypeBridgeStatus::Ok,
        );
        let path_text_alias = unsafe {
            path.primary
                .data
                .add(1)
                .cast_mut()
                .cast::<TypeBridgeExecutionDiagnosticDetailViewV1>()
        };
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_get_v1(diagnostics, 0, 0, path_text_alias)
            },
            TypeBridgeStatus::InvalidArgument,
        );

        let mut signed_detail = empty_detail_view();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_get_v1(
                    diagnostics,
                    0,
                    0,
                    &mut signed_detail,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            signed_detail.kind,
            TypeBridgeExecutionDiagnosticDetailKind::Signed
        );
        let signed_key_alias = unsafe { signed_detail.key.data.add(1).cast_mut().cast::<i64>() };
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_signed(diagnostics, 0, 0, signed_key_alias)
            },
            TypeBridgeStatus::InvalidArgument,
        );

        let mut signed = i64::MAX;
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_signed(diagnostics, 0, 0, &mut signed)
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(signed, -17);

        let mut list_item = empty_view();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_list_get(
                    diagnostics,
                    0,
                    1,
                    0,
                    &mut list_item,
                )
            },
            TypeBridgeStatus::Ok,
        );
        let list_count_alias = unsafe { list_item.data.add(1).cast_mut().cast::<usize>() };
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_list_count(
                    diagnostics,
                    0,
                    1,
                    list_count_alias,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );
        let list_view_alias = unsafe {
            list_item
                .data
                .add(list_item.length - 1)
                .cast_mut()
                .cast::<TypeBridgeByteView>()
        };
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_list_get(
                    diagnostics,
                    0,
                    1,
                    1,
                    list_view_alias,
                )
            },
            TypeBridgeStatus::InvalidArgument,
        );

        // All views and the owner remain intact and reusable after every
        // hostile overlap, including list storage reached through a child view.
        let mut list_count = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_list_count(
                    diagnostics,
                    0,
                    1,
                    &mut list_count,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(list_count, 2);
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_list_get(
                    diagnostics,
                    0,
                    1,
                    1,
                    &mut list_item,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(list_item.data, list_item.length) },
            b"second_binding_identity",
        );
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_close(&mut diagnostics) },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
    }

    #[test]
    fn additive_acceptance_path_and_detail_numeric_values_are_frozen() {
        assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Argument as i32, 1);
        assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Role as i32, 5);
        assert_eq!(
            TypeBridgeExecutionDiagnosticPathKind::QueryRequest as i32,
            6
        );
        assert_eq!(
            TypeBridgeExecutionDiagnosticPathKind::QueryOutputName as i32,
            18
        );
        assert_eq!(
            TypeBridgeExecutionDiagnosticPathKind::ContractField as i32,
            19
        );
        assert_eq!(
            TypeBridgeExecutionDiagnosticPathKind::ContractIdentity as i32,
            20
        );
        assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Unknown as i32, 255);
        assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Boolean as i32, 1);
        assert_eq!(
            TypeBridgeExecutionDiagnosticDetailKind::CommitOutcome as i32,
            11
        );
        assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Text as i32, 12);
        assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Signed as i32, 13);
        assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::TextList as i32, 14);
        assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Unknown as i32, 255);
    }
}
