//! Borrowed cursors over the committed plan answer; metadata is prepared at commit.

use std::collections::BTreeMap;
use std::mem::{size_of, size_of_val};
use std::ptr;
use std::sync::{Arc, OnceLock};

use serde_json::Value;
use type_bridge_contract::query_plan::v3::{FetchDocumentShape, QueryPlanV3, Stage};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::session::staged::{Pending, PlanAnswer};

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus, guarded};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::staged::{TypeBridgePending, answer_status};

const INVALID: TypeBridgeStatus = TypeBridgeStatus::InvalidArgument;
const EMPTY_VIEW: TypeBridgeByteView = TypeBridgeByteView {
    data: ptr::null(),
    length: 0,
};

/// Scalar kinds match the existing projected-value kinds; zero is absent.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeValueKind {
    Absent = 0,
    String = 1,
    Long = 2,
    Double = 3,
    Boolean = 4,
    Date = 5,
    DateTime = 6,
    DateTimeTz = 7,
    Decimal = 8,
    Duration = 9,
    List = 10,
    Thing = 11,
}

fn kind(domain: ValueTypeTag) -> TypeBridgeValueKind {
    match domain {
        ValueTypeTag::String => TypeBridgeValueKind::String,
        ValueTypeTag::Long => TypeBridgeValueKind::Long,
        ValueTypeTag::Double => TypeBridgeValueKind::Double,
        ValueTypeTag::Boolean => TypeBridgeValueKind::Boolean,
        ValueTypeTag::Date => TypeBridgeValueKind::Date,
        ValueTypeTag::DateTime => TypeBridgeValueKind::DateTime,
        ValueTypeTag::DateTimeTz => TypeBridgeValueKind::DateTimeTz,
        ValueTypeTag::Decimal => TypeBridgeValueKind::Decimal,
        ValueTypeTag::Duration => TypeBridgeValueKind::Duration,
    }
}

fn named_kind(name: &str) -> Result<TypeBridgeValueKind, TypeBridgeStatus> {
    Ok(match name {
        "string" => TypeBridgeValueKind::String,
        "long" | "integer" => TypeBridgeValueKind::Long,
        "double" => TypeBridgeValueKind::Double,
        "boolean" => TypeBridgeValueKind::Boolean,
        "date" => TypeBridgeValueKind::Date,
        "datetime" => TypeBridgeValueKind::DateTime,
        "datetime_tz" | "datetime-tz" => TypeBridgeValueKind::DateTimeTz,
        "decimal" => TypeBridgeValueKind::Decimal,
        "duration" => TypeBridgeValueKind::Duration,
        _ => return Err(INVALID),
    })
}

pub(crate) struct PendingState {
    pub(crate) pending: PendingSource,
    shape: Option<Arc<FetchDocumentShape>>,
    roles: usize,
    views: OnceLock<AnswerViews>,
}

pub(crate) enum PendingSource {
    Raw(Pending<PlanAnswer>),
    Authored(Box<crate::plan_builder::selections::AuthoredPending>),
    Query(Box<crate::query::StagedQueryPending>),
}
impl PendingSource {
    pub(crate) fn get(
        &self,
    ) -> Result<Arc<PlanAnswer>, type_bridge_orm::session::staged::PendingError> {
        match self {
            Self::Raw(pending) => pending.get(),
            Self::Authored(pending) => pending.answer(),
            Self::Query(_) => Err(type_bridge_orm::session::staged::PendingError::InvalidPlan(
                Box::new(type_bridge_contract::diagnostic::Diagnostic::new(
                    type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
                    type_bridge_contract::diagnostic::DiagnosticCode::new(
                        "c_query_pending_answer_kind",
                    )
                    .expect("static diagnostic code"),
                    "A typed query pending requires the query result accessor",
                )),
            )),
        }
    }
}

impl PendingState {
    pub(crate) fn query(pending: crate::query::StagedQueryPending) -> Self {
        Self {
            pending: PendingSource::Query(Box::new(pending)),
            shape: None,
            roles: 0,
            views: OnceLock::new(),
        }
    }

    pub(crate) fn new(pending: Pending<PlanAnswer>, plan: &QueryPlanV3) -> Self {
        Self {
            pending: PendingSource::Raw(pending),
            shape: plan.fetch_document_shape().cloned().map(Arc::new),
            roles: plan
                .pipeline()
                .iter()
                .find_map(|stage| match stage {
                    Stage::Fetch(doc) => Some(doc.roles.len()),
                    _ => None,
                })
                .unwrap_or(0),
            views: OnceLock::new(),
        }
    }

    pub(crate) fn authored(pending: crate::plan_builder::selections::AuthoredPending) -> Self {
        let plan = pending.selection_plan();
        let shape = plan.fetch_document_shape().cloned().map(Arc::new);
        let roles = plan
            .pipeline()
            .iter()
            .find_map(|stage| match stage {
                Stage::Fetch(doc) => Some(doc.roles.len()),
                _ => None,
            })
            .unwrap_or(0);
        Self {
            pending: PendingSource::Authored(Box::new(pending)),
            shape,
            roles,
            views: OnceLock::new(),
        }
    }

    pub(crate) fn prepare(&self) {
        let Ok(answer) = self.pending.get() else {
            return;
        };
        let _ = self
            .views
            .set(AnswerViews::new(answer, self.shape.clone(), self.roles));
    }

    fn views(&self) -> Result<&AnswerViews, TypeBridgeStatus> {
        let answer = self.pending.get().map_err(|error| answer_status(&error))?;
        // A reader may arrive between commit and prepare, or after a commit
        // that panicked past resolving the answer; build the views here then.
        Ok(self
            .views
            .get_or_init(|| AnswerViews::new(answer, self.shape.clone(), self.roles)))
    }
}

enum AnswerViews {
    Documents(Vec<TypeBridgeDocument>),
    Rows(Vec<TypeBridgeRow>),
    Scalar,
}

impl AnswerViews {
    fn new(answer: Arc<PlanAnswer>, shape: Option<Arc<FetchDocumentShape>>, roles: usize) -> Self {
        let storage = Arc::new(AnswerStorage {
            answer,
            shape,
            roles,
            ranges: OnceLock::new(),
        });
        let views = match storage.answer.as_ref() {
            PlanAnswer::Documents(documents) => AnswerViews::Documents(
                (0..documents.len())
                    .map(|index| TypeBridgeDocument::new(Arc::clone(&storage), index, None))
                    .collect(),
            ),
            PlanAnswer::Rows(rows) => AnswerViews::Rows(
                (0..rows.len())
                    .map(|index| TypeBridgeRow {
                        tag: ROW_TAG,
                        storage: Arc::clone(&storage),
                        index,
                    })
                    .collect(),
            ),
            _ => AnswerViews::Scalar,
        };
        let mut ranges = Vec::new();
        match &views {
            AnswerViews::Documents(documents) => {
                for doc in documents {
                    doc.ranges(&mut ranges);
                }
            }
            AnswerViews::Rows(rows) => {
                ranges.push((rows.as_ptr() as usize, size_of_val(rows.as_slice())))
            }
            AnswerViews::Scalar => {}
        }
        let _ = storage.ranges.set(ranges);
        views
    }
}

struct AnswerStorage {
    answer: Arc<PlanAnswer>,
    shape: Option<Arc<FetchDocumentShape>>,
    roles: usize,
    ranges: OnceLock<Vec<(usize, usize)>>,
}

impl AnswerStorage {
    fn fence(&self, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        if let Some(ranges) = self.ranges.get() {
            for &(pointer, length) in ranges {
                preflight.check_bytes(pointer as *const _, length)?;
            }
        }
        check_answer(preflight, &self.answer)
    }
}

/// Opaque document view borrowed until its pending handle is freed.
#[repr(C)]
pub struct TypeBridgeDocument {
    tag: u64,
    storage: Arc<AnswerStorage>,
    index: usize,
    role: Option<usize>,
    iid_key: String,
    type_key: String,
    attributes_key: String,
    players: BTreeMap<usize, TypeBridgeDocument>,
}

impl TypeBridgeDocument {
    fn new(storage: Arc<AnswerStorage>, index: usize, role: Option<usize>) -> Self {
        let key =
            |name| role.map_or_else(|| format!("_{name}"), |role| format!("_role_{role}_{name}"));
        let attributes_key = role.map_or_else(
            || "attributes".to_owned(),
            |role| format!("_role_{role}_attributes"),
        );
        let players = if role.is_none() {
            let PlanAnswer::Documents(documents) = storage.answer.as_ref() else {
                unreachable!()
            };
            documents[index]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(key, value)| {
                    let role = key
                        .strip_prefix("_role_")?
                        .strip_suffix("_iid")?
                        .parse::<usize>()
                        .ok()?;
                    if role >= storage.roles || value.is_null() {
                        return None;
                    }
                    Some((role, Self::new(Arc::clone(&storage), index, Some(role))))
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        Self {
            tag: DOCUMENT_TAG,
            iid_key: key("iid"),
            type_key: key("type"),
            attributes_key,
            storage,
            index,
            role,
            players,
        }
    }

    fn ranges(&self, ranges: &mut Vec<(usize, usize)>) {
        ranges.push((std::ptr::from_ref(self) as usize, size_of_val(self)));
        for key in [&self.iid_key, &self.type_key, &self.attributes_key] {
            ranges.push((key.as_ptr() as usize, key.len()));
        }
        for player in self.players.values() {
            player.ranges(ranges);
        }
    }

    fn raw(&self) -> &Value {
        let PlanAnswer::Documents(documents) = self.storage.answer.as_ref() else {
            unreachable!()
        };
        &documents[self.index]
    }

    fn domain(&self, label: &str) -> Result<TypeBridgeValueKind, TypeBridgeStatus> {
        self.storage
            .shape
            .as_ref()
            .and_then(|shape| {
                shape.attribute_domain(self.role, self.raw().get(&self.type_key)?.as_str()?, label)
            })
            .map(kind)
            .ok_or(INVALID)
    }

    fn attribute(&self, label: &str, index: usize) -> Result<Scalar<'_>, TypeBridgeStatus> {
        let kind = self.domain(label)?;
        let value = self
            .raw()
            .get(&self.attributes_key)
            .and_then(|attrs| attrs.get(label))
            .ok_or(INVALID)?;
        let value = match value.as_array() {
            Some(values) => values.get(index).ok_or(INVALID)?,
            None if index == 0 && !value.is_null() => value,
            None => return Err(INVALID),
        };
        Ok(Scalar {
            kind,
            value,
            canonical: false,
        })
    }
}

/// Opaque row view borrowed until its pending handle is freed.
#[repr(C)]
pub struct TypeBridgeRow {
    tag: u64,
    storage: Arc<AnswerStorage>,
    index: usize,
}

impl TypeBridgeRow {
    fn raw(&self) -> &Value {
        let PlanAnswer::Rows(rows) = self.storage.answer.as_ref() else {
            unreachable!()
        };
        &rows[self.index]
    }

    fn scalar(&self, variable: &str) -> Result<Scalar<'_>, TypeBridgeStatus> {
        let concept = self.raw().get(variable).ok_or(INVALID)?;
        if concept.is_array() {
            return Ok(Scalar {
                kind: TypeBridgeValueKind::List,
                value: concept,
                canonical: false,
            });
        }
        if concept.is_null() || concept.get("kind").and_then(Value::as_str) == Some("absent") {
            return Ok(Scalar {
                kind: TypeBridgeValueKind::Absent,
                value: concept,
                canonical: false,
            });
        }
        if matches!(concept.get("kind").and_then(Value::as_str), Some("thing"))
            || matches!(
                concept.get("category").and_then(Value::as_str),
                Some("entity" | "relation" | "Entity" | "Relation")
            )
        {
            return Ok(Scalar {
                kind: TypeBridgeValueKind::Thing,
                value: concept,
                canonical: false,
            });
        }
        if let Some(domain) = concept.get("value_type").and_then(Value::as_str) {
            return Ok(Scalar {
                kind: named_kind(domain)?,
                value: concept.get("value").ok_or(INVALID)?,
                canonical: false,
            });
        }
        let value = concept.get("value").ok_or(INVALID)?;
        let kind = named_kind(value.get("kind").and_then(Value::as_str).ok_or(INVALID)?)?;
        Ok(Scalar {
            kind,
            value: value
                .get(if kind == TypeBridgeValueKind::Double {
                    "bits"
                } else {
                    "value"
                })
                .ok_or(INVALID)?,
            canonical: true,
        })
    }
}

struct Scalar<'a> {
    kind: TypeBridgeValueKind,
    value: &'a Value,
    canonical: bool,
}
impl Scalar<'_> {
    fn require(&self, expected: TypeBridgeValueKind) -> Result<(), TypeBridgeStatus> {
        if self.kind == expected {
            Ok(())
        } else {
            Err(INVALID)
        }
    }
    fn text(&self, expected: TypeBridgeValueKind) -> Result<TypeBridgeByteView, TypeBridgeStatus> {
        self.require(expected)?;
        Ok(view(self.value.as_str().ok_or(INVALID)?))
    }
    fn long(&self) -> Result<i64, TypeBridgeStatus> {
        self.require(TypeBridgeValueKind::Long)?;
        if self.canonical {
            self.value
                .as_str()
                .ok_or(INVALID)?
                .parse()
                .map_err(|_| INVALID)
        } else {
            self.value
                .as_i64()
                .or_else(|| self.value.as_str()?.parse().ok())
                .ok_or(INVALID)
        }
    }
    fn double(&self) -> Result<u64, TypeBridgeStatus> {
        self.require(TypeBridgeValueKind::Double)?;
        if self.canonical {
            u64::from_str_radix(self.value.as_str().ok_or(INVALID)?, 16).map_err(|_| INVALID)
        } else {
            self.value.as_f64().map(f64::to_bits).ok_or(INVALID)
        }
    }
    fn boolean(&self) -> Result<u8, TypeBridgeStatus> {
        self.require(TypeBridgeValueKind::Boolean)?;
        self.value.as_bool().map(u8::from).ok_or(INVALID)
    }
}

fn view(value: &str) -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: value.as_ptr(),
        length: value.len(),
    }
}

fn check_json(preflight: &DirectOutputPreflight, value: &Value) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes((value as *const Value).cast(), size_of_val(value))?;
    match value {
        Value::String(text) => preflight.check_bytes(text.as_ptr().cast(), text.len())?,
        Value::Array(values) => {
            preflight.check_bytes(values.as_ptr().cast(), size_of_val(values.as_slice()))?;
            for value in values {
                check_json(preflight, value)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                preflight.check_bytes(key.as_ptr().cast(), key.len())?;
                check_json(preflight, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_answer(
    preflight: &DirectOutputPreflight,
    answer: &PlanAnswer,
) -> Result<(), TypeBridgeStatus> {
    if let PlanAnswer::Documents(values) | PlanAnswer::Rows(values) = answer {
        preflight.check_bytes(values.as_ptr().cast(), size_of_val(values.as_slice()))?;
        for value in values {
            check_json(preflight, value)?;
        }
    }
    Ok(())
}

pub(crate) const PENDING_TAG: u64 = 0x74625f70656e6469;
pub(crate) const QUERY_PENDING_TAG: u64 = 0x74625f7170656e64;
const DOCUMENT_TAG: u64 = 0x74625f646f63756d;
const ROW_TAG: u64 = 0x74625f726f775f5f;

trait Cursor {
    const TAG: u64;
    fn fence(&self, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus>;
}
impl Cursor for TypeBridgeDocument {
    const TAG: u64 = DOCUMENT_TAG;
    fn fence(&self, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        self.storage.fence(preflight)
    }
}
impl Cursor for TypeBridgeRow {
    const TAG: u64 = ROW_TAG;
    fn fence(&self, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        self.storage.fence(preflight)
    }
}
impl Cursor for TypeBridgePending {
    const TAG: u64 = PENDING_TAG;
    fn fence(&self, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        match &self.state.pending {
            PendingSource::Authored(pending) => {
                preflight.check_package_borrowed_ranges(&pending.source.package)?;
                if let Some(Ok(result)) = pending.model_result.get() {
                    result.fence(preflight)?;
                }
                if let Some(Ok(result)) = pending.rows_result.get() {
                    result.fence(preflight)?;
                }
            }
            PendingSource::Query(pending) => pending.check_borrowed_ranges(preflight)?,
            PendingSource::Raw(_) => {}
        }
        if let Some(views) = self.state.views.get() {
            match views {
                AnswerViews::Documents(docs) => {
                    if let Some(doc) = docs.first() {
                        doc.fence(preflight)?;
                    }
                }
                AnswerViews::Rows(rows) => {
                    if let Some(row) = rows.first() {
                        row.fence(preflight)?;
                    }
                }
                AnswerViews::Scalar => {}
            }
        }
        Ok(())
    }
}

unsafe fn fence_as<C: Cursor>(
    cursor: *const u8,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(cursor.cast(), size_of::<C>())?;
    // SAFETY: the checked tag identifies the live cursor's actual kind.
    unsafe { (&*cursor.cast::<C>()).fence(preflight) }
}

/// Fence the actual cursor kind before any output is cleared, including on a wrong-kind call.
///
/// # Safety
/// Non-null inputs are live cursors; outputs have passed direct output preflight.
pub(crate) unsafe fn check_cursor_ranges(
    preflight: &DirectOutputPreflight,
    cursor: *const u8,
    declared_size: usize,
) -> Result<Option<u64>, TypeBridgeStatus> {
    if cursor.is_null() {
        return Ok(None);
    }
    preflight.check_bytes(cursor.cast(), size_of::<u64>())?;
    // SAFETY: every live cursor starts with its initialized kind tag.
    let tag = unsafe { cursor.cast::<u64>().read_unaligned() };
    // SAFETY: each recognized tag selects that cursor's actual layout.
    unsafe {
        match tag {
            PENDING_TAG | QUERY_PENDING_TAG => fence_as::<TypeBridgePending>(cursor, preflight),
            DOCUMENT_TAG => fence_as::<TypeBridgeDocument>(cursor, preflight),
            ROW_TAG => fence_as::<TypeBridgeRow>(cursor, preflight),
            _ => preflight.check_bytes(cursor.cast(), declared_size),
        }?;
    }
    Ok(Some(tag))
}

/// # Safety
/// Non-null pointers are live cursors, possibly of another kind; output slots and
/// label bytes are writable/readable for the call. No concurrent owner mutation.
unsafe fn read<C: Cursor, T: Copy>(
    cursor: *const C,
    label: TypeBridgeByteView,
    out: *mut T,
    empty: T,
    call: impl FnOnce(&C, &str) -> Result<T, TypeBridgeStatus>,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight = match direct_output_preflight(&[(out.cast(), size_of::<T>())]) {
            Ok(p) => p,
            Err(status) => return status,
        };
        // SAFETY: the caller retains a live cursor through this call.
        let tag = match unsafe { check_cursor_ranges(&preflight, cursor.cast(), size_of::<C>()) } {
            Ok(tag) => tag,
            Err(status) => return status,
        };
        let valid = tag == Some(C::TAG);
        if !label.data.is_null()
            && let Err(status) = preflight.check_bytes(label.data.cast(), label.length)
        {
            return status;
        }
        if !out.is_null() {
            unsafe {
                out.write_unaligned(empty);
            }
        }
        if !valid || out.is_null() || (label.data.is_null() && label.length != 0) {
            return INVALID;
        }
        let bytes = if label.length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(label.data, label.length) }
        };
        let Ok(label) = std::str::from_utf8(bytes) else {
            return INVALID;
        };
        match call(unsafe { &*cursor }, label) {
            Ok(value) => {
                unsafe {
                    out.write_unaligned(value);
                }
                TypeBridgeStatus::Ok
            }
            Err(status) => status,
        }
    })
}

macro_rules! pending_cursor {
    ($count:ident, $at:ident, $variant:ident, $cursor:ty) => {
        /// Number of values of the corresponding committed answer kind.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $count(
            pending: *const TypeBridgePending,
            out: *mut usize,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded live-handle and output-slot contract.
            unsafe {
                read(pending, EMPTY_VIEW, out, 0, |pending, _| {
                    match pending.state.views()? {
                        AnswerViews::$variant(values) => Ok(values.len()),
                        _ => Err(INVALID),
                    }
                })
            }
        }
        /// Borrow one value until the pending handle is freed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $at(
            pending: *const TypeBridgePending,
            index: usize,
            out: *mut *const $cursor,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded live-handle and output-slot contract.
            unsafe {
                read(
                    pending,
                    EMPTY_VIEW,
                    out,
                    ptr::null(),
                    |pending, _| match pending.state.views()? {
                        AnswerViews::$variant(values) => {
                            values.get(index).map(std::ptr::from_ref).ok_or(INVALID)
                        }
                        _ => Err(INVALID),
                    },
                )
            }
        }
    };
}
pending_cursor!(
    type_bridge_pending_document_count,
    type_bridge_pending_document,
    Documents,
    TypeBridgeDocument
);
pending_cursor!(
    type_bridge_pending_row_count,
    type_bridge_pending_row,
    Rows,
    TypeBridgeRow
);

macro_rules! document_text {
    ($name:ident, $key:ident) => {
        /// Borrow a document identity string until its pending handle is freed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            doc: *const TypeBridgeDocument,
            out: *mut TypeBridgeByteView,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded live-cursor and output-slot contract.
            unsafe {
                read(doc, EMPTY_VIEW, out, EMPTY_VIEW, |doc, _| {
                    Ok(view(
                        doc.raw()
                            .get(&doc.$key)
                            .and_then(Value::as_str)
                            .ok_or(INVALID)?,
                    ))
                })
            }
        }
    };
}
document_text!(type_bridge_document_iid, iid_key);
document_text!(type_bridge_document_type, type_key);

/// Number of scalar values of a schema-owned attribute; an absent one has zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_document_attribute_count(
    doc: *const TypeBridgeDocument,
    label: TypeBridgeByteView,
    out: *mut usize,
) -> TypeBridgeStatus {
    // SAFETY: forwarded live-cursor, label and output-slot contract.
    unsafe {
        read(doc, label, out, 0, |doc, label| {
            doc.domain(label)?;
            Ok(
                match doc
                    .raw()
                    .get(&doc.attributes_key)
                    .and_then(|attrs| attrs.get(label))
                {
                    None | Some(Value::Null) => 0,
                    Some(Value::Array(values)) => values.len(),
                    Some(_) => 1,
                },
            )
        })
    }
}

/// Kind of one attribute element, before selecting its typed getter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_document_attribute_kind(
    doc: *const TypeBridgeDocument,
    label: TypeBridgeByteView,
    index: usize,
    out: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded live-cursor, label and output-slot contract.
    unsafe {
        read(doc, label, out, 0, |doc, label| {
            Ok(doc.attribute(label, index)?.kind as u32)
        })
    }
}

/// Borrow a listed role player, or return null for an absent optional player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_document_role(
    doc: *const TypeBridgeDocument,
    index: usize,
    out: *mut *const TypeBridgeDocument,
) -> TypeBridgeStatus {
    // SAFETY: forwarded live-cursor and output-slot contract.
    unsafe {
        read(doc, EMPTY_VIEW, out, ptr::null(), |doc, _| {
            if doc.role.is_some() || index >= doc.storage.roles {
                return Err(INVALID);
            }
            Ok(doc
                .players
                .get(&index)
                .map_or(ptr::null(), std::ptr::from_ref))
        })
    }
}

/// Kind of a projected variable (including absent, thing and list).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_row_value_kind(
    row: *const TypeBridgeRow,
    variable: TypeBridgeByteView,
    out: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded live-cursor, variable and output-slot contract.
    unsafe {
        read(row, variable, out, 0, |row, variable| {
            Ok(row.scalar(variable)?.kind as u32)
        })
    }
}

macro_rules! scalar_getters {
    ($doc_fn:ident, $row_fn:ident, $out:ty, $empty:expr, $get:expr) => {
        /// Read one attribute element; wrong kind and bounds are INVALID_ARGUMENT.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $doc_fn(
            doc: *const TypeBridgeDocument,
            label: TypeBridgeByteView,
            index: usize,
            out: *mut $out,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded live-cursor, label and output-slot contract.
            unsafe {
                read(doc, label, out, $empty, |doc, label| {
                    ($get)(doc.attribute(label, index)?)
                })
            }
        }
        /// Read one projected scalar; wrong kind and absent values are INVALID_ARGUMENT.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $row_fn(
            row: *const TypeBridgeRow,
            variable: TypeBridgeByteView,
            out: *mut $out,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded live-cursor, variable and output-slot contract.
            unsafe {
                read(row, variable, out, $empty, |row, variable| {
                    ($get)(row.scalar(variable)?)
                })
            }
        }
    };
}
scalar_getters!(
    type_bridge_document_attribute_string,
    type_bridge_row_value_string,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::String)
);
scalar_getters!(
    type_bridge_document_attribute_long,
    type_bridge_row_value_long,
    i64,
    0,
    |scalar: Scalar<'_>| scalar.long()
);
scalar_getters!(
    type_bridge_document_attribute_double,
    type_bridge_row_value_double,
    u64,
    0,
    |scalar: Scalar<'_>| scalar.double()
);
scalar_getters!(
    type_bridge_document_attribute_boolean,
    type_bridge_row_value_boolean,
    u8,
    0,
    |scalar: Scalar<'_>| scalar.boolean()
);
scalar_getters!(
    type_bridge_document_attribute_date,
    type_bridge_row_value_date,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::Date)
);
scalar_getters!(
    type_bridge_document_attribute_datetime,
    type_bridge_row_value_datetime,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::DateTime)
);
scalar_getters!(
    type_bridge_document_attribute_datetime_tz,
    type_bridge_row_value_datetime_tz,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::DateTimeTz)
);
scalar_getters!(
    type_bridge_document_attribute_decimal,
    type_bridge_row_value_decimal,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::Decimal)
);
scalar_getters!(
    type_bridge_document_attribute_duration,
    type_bridge_row_value_duration,
    TypeBridgeByteView,
    EMPTY_VIEW,
    |scalar: Scalar<'_>| scalar.text(TypeBridgeValueKind::Duration)
);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
    use type_bridge_contract::query_plan::v3::{DocumentRole, PlanBuilder};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents};

    pub(crate) fn plan() -> QueryPlanV3 {
        let source = r#"format: typebridge.schema/v2
attributes:
  key: { value: string }
  measure-long: { value: integer }
  measure-double: { value: double }
  enabled: { value: boolean }
  born-on: { value: date }
  observed-at: { value: datetime }
  zoned-at: { value: datetime-tz }
  balance: { value: decimal }
  elapsed: { value: duration }
entities:
  person:
    owns: { key: { card: { min: 0, max: 2 } }, measure-long: {}, measure-double: {}, enabled: {}, born-on: {}, observed-at: {}, zoned-at: {}, balance: {}, elapsed: {} }
relations:
  membership:
    owns: { key: {} }
    relates: { member: {}, optional: {} }
plays:
  person:
    membership: { member: {}, optional: {} }
"#;
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("answers.yaml").unwrap(), source)]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
            b"cursor-tests",
        )
        .unwrap();
        let mut builder = PlanBuilder::new();
        let thing = builder.exact(TypeId::new(TypeKind::Relation, "membership").unwrap());
        builder.fetch(
            thing,
            vec![
                DocumentRole::new(RoleId::new("membership", "member").unwrap(), true),
                DocumentRole::new(RoleId::new("membership", "optional").unwrap(), false),
            ],
        );
        builder.build(&declared, semantics).unwrap()
    }

    pub(crate) fn answer() -> Value {
        json!({"_iid":"0x01", "_type":"membership", "attributes":{"key":"group"},
        "_role_0_iid":"0x02", "_role_0_type":"person", "_role_0_attributes":{
            "key":["Ada", "Grace"], "measure-long":i64::MIN, "measure-double":1.25, "enabled":true,
            "born-on":"2026-10-26", "observed-at":"2026-10-26T01:02:03",
            "zoned-at":"2026-10-26T01:02:03Z", "balance":"12.34", "elapsed":"P1D"
        }})
    }

    unsafe fn text(value: TypeBridgeByteView) -> String {
        // SAFETY: the test keeps the owning answer alive.
        String::from_utf8(unsafe { std::slice::from_raw_parts(value.data, value.length) }.to_vec())
            .unwrap()
    }

    #[test]
    fn document_cursors_read_every_kind_and_optional_players_and_fence_aliases() {
        let plan = plan();
        let answer = Arc::new(PlanAnswer::Documents(vec![answer()]));
        answer.to_staged_v3(&plan).unwrap();
        let views = AnswerViews::new(
            answer,
            plan.fetch_document_shape().cloned().map(Arc::new),
            2,
        );
        let AnswerViews::Documents(docs) = views else {
            panic!()
        };
        let doc = &docs[0];
        assert_eq!(
            doc.players.len(),
            1,
            "absent players retain no cursor allocation"
        );
        // SAFETY: live borrowed cursors, readable labels and distinct writable outputs.
        unsafe {
            let mut result = EMPTY_VIEW;
            assert_eq!(
                type_bridge_document_iid(doc, &mut result),
                TypeBridgeStatus::Ok
            );
            assert_eq!(text(result), "0x01");
            assert_eq!(
                type_bridge_document_type(doc, &mut result),
                TypeBridgeStatus::Ok
            );
            assert_eq!(text(result), "membership");
            let mut player = ptr::null();
            assert_eq!(
                type_bridge_document_role(doc, 0, &mut player),
                TypeBridgeStatus::Ok
            );
            assert!(!player.is_null());
            let mut absent = ptr::dangling();
            assert_eq!(
                type_bridge_document_role(doc, 1, &mut absent),
                TypeBridgeStatus::Ok
            );
            assert!(absent.is_null());
            assert_eq!(type_bridge_document_role(doc, 2, &mut absent), INVALID);
            let mut count = usize::MAX;
            assert_eq!(
                type_bridge_document_attribute_count(player, view("key"), &mut count),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, 2);
            assert_eq!(
                type_bridge_document_attribute_string(player, view("key"), 1, &mut result),
                TypeBridgeStatus::Ok
            );
            assert_eq!(text(result), "Grace");
            let mut long = 0;
            assert_eq!(
                type_bridge_document_attribute_long(player, view("measure-long"), 0, &mut long),
                TypeBridgeStatus::Ok
            );
            assert_eq!(long, i64::MIN);
            let mut double = 0;
            assert_eq!(
                type_bridge_document_attribute_double(
                    player,
                    view("measure-double"),
                    0,
                    &mut double
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(f64::from_bits(double), 1.25);
            let mut boolean = 0;
            assert_eq!(
                type_bridge_document_attribute_boolean(player, view("enabled"), 0, &mut boolean),
                TypeBridgeStatus::Ok
            );
            assert_eq!(boolean, 1);
            type TextGetter = unsafe extern "C" fn(
                *const TypeBridgeDocument,
                TypeBridgeByteView,
                usize,
                *mut TypeBridgeByteView,
            ) -> TypeBridgeStatus;
            for (label, getter, expected, expected_kind) in [
                (
                    "born-on",
                    type_bridge_document_attribute_date as TextGetter,
                    "2026-10-26",
                    TypeBridgeValueKind::Date,
                ),
                (
                    "observed-at",
                    type_bridge_document_attribute_datetime,
                    "2026-10-26T01:02:03",
                    TypeBridgeValueKind::DateTime,
                ),
                (
                    "zoned-at",
                    type_bridge_document_attribute_datetime_tz,
                    "2026-10-26T01:02:03Z",
                    TypeBridgeValueKind::DateTimeTz,
                ),
                (
                    "balance",
                    type_bridge_document_attribute_decimal,
                    "12.34",
                    TypeBridgeValueKind::Decimal,
                ),
                (
                    "elapsed",
                    type_bridge_document_attribute_duration,
                    "P1D",
                    TypeBridgeValueKind::Duration,
                ),
            ] {
                assert_eq!(
                    getter(player, view(label), 0, &mut result),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(text(result), expected);
                let mut kind = 0;
                assert_eq!(
                    type_bridge_document_attribute_kind(player, view(label), 0, &mut kind),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(kind, expected_kind as u32);
            }
            long = 99;
            assert_eq!(
                type_bridge_document_attribute_long(player, view("key"), 0, &mut long),
                INVALID
            );
            assert_eq!(long, 0);
            assert_eq!(
                type_bridge_document_attribute_string(player, view("key"), 2, &mut result),
                INVALID
            );
            assert!(result.data.is_null() && result.length == 0);
            assert_eq!(
                type_bridge_document_attribute_string(ptr::null(), view("key"), 0, &mut result),
                INVALID
            );
            assert_eq!(
                type_bridge_document_attribute_count(player, view("unknown"), &mut count),
                INVALID
            );
            assert_eq!(count, 0);
            // All root/player handles and borrowed strings remain immutable.
            assert_eq!(
                type_bridge_document_attribute_long(
                    player,
                    view("measure-long"),
                    0,
                    (doc as *const TypeBridgeDocument).cast_mut().cast()
                ),
                INVALID
            );
            assert_eq!(
                type_bridge_document_attribute_string(player, view("key"), 0, &mut result),
                TypeBridgeStatus::Ok
            );
            let before = text(result);
            assert_eq!(
                type_bridge_document_attribute_boolean(
                    player,
                    view("enabled"),
                    0,
                    result.data.cast_mut()
                ),
                INVALID
            );
            assert_eq!(text(result), before);
            assert_eq!(
                type_bridge_document_attribute_string(
                    player,
                    view("key"),
                    0,
                    view("key").data.cast_mut().cast()
                ),
                INVALID
            );
            assert_eq!(
                type_bridge_document_attribute_string(player, view("key"), 1, &mut result),
                TypeBridgeStatus::Ok
            );
            let before = text(result);
            assert_eq!(
                type_bridge_row_value_kind(
                    player.cast(),
                    view("key"),
                    result.data.cast_mut().cast()
                ),
                INVALID
            );
            assert_eq!(
                text(result),
                before,
                "foreign row getters must fence document bytes"
            );
            let mut legacy_value = 99;
            assert_eq!(
                crate::staged::type_bridge_pending_answer(
                    player.cast(),
                    result.data.cast_mut().cast(),
                    &mut legacy_value
                ),
                INVALID
            );
            assert_eq!(text(result), before);
            assert_eq!(
                legacy_value, 99,
                "aliased legacy outputs must stay untouched"
            );
            let key_pointer = (*player).type_key.as_ptr();
            let before_key = (*player).type_key.clone();
            assert_eq!(
                type_bridge_pending_row_count(player.cast(), key_pointer.cast_mut().cast()),
                INVALID
            );
            assert_eq!(
                (*player).type_key,
                before_key,
                "foreign pending getters must fence cursor keys"
            );
            let mut unaligned = [0xffu8; 9];
            assert_eq!(
                type_bridge_document_attribute_long(
                    player,
                    view("measure-long"),
                    0,
                    unaligned.as_mut_ptr().add(1).cast()
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(unaligned[1..], i64::MIN.to_ne_bytes());
        }
    }

    #[test]
    fn row_cursors_read_provider_and_proxy_values_and_reject_foreign_handles() {
        let mut values = [
            json!({"text":{"category":"value","value_type":"string","value":"Ada"}, "long":{"category":"value","value_type":"integer","value":-7}, "double":{"category":"value","value_type":"double","value":1.25}, "boolean":{"category":"value","value_type":"boolean","value":true}}),
            json!({"text":{"kind":"value","value":{"kind":"string","value":"Ada"}}, "long":{"kind":"value","value":{"kind":"long","value":"-7"}}, "double":{"kind":"value","value":{"kind":"double","bits":"3ff4000000000000"}}, "boolean":{"kind":"attribute","type_id":{"kind":"attribute","label":"flag"},"value":{"kind":"boolean","value":true}}}),
        ];
        for (name, provider, text) in [
            ("date", "date", "2026-10-26"),
            ("datetime", "datetime", "2026-10-26T01:02:03"),
            ("datetime_tz", "datetime-tz", "2026-10-26T01:02:03Z"),
            ("decimal", "decimal", "12.34"),
            ("duration", "duration", "P1D"),
        ] {
            values[0][name] = json!({"category":"value", "value_type":provider, "value":text});
            values[1][name] = json!({"kind":"value", "value":{"kind":name, "value":text}});
        }
        values[0]["absent"] = Value::Null;
        values[1]["absent"] = json!({"kind":"absent"});
        let views = AnswerViews::new(Arc::new(PlanAnswer::Rows(values.into())), None, 0);
        let AnswerViews::Rows(rows) = views else {
            panic!()
        };
        for row in &rows {
            // SAFETY: live borrowed row and writable output slots.
            unsafe {
                let mut text_value = EMPTY_VIEW;
                assert_eq!(
                    type_bridge_row_value_string(row, view("text"), &mut text_value),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(text(text_value), "Ada");
                let mut long = 0;
                assert_eq!(
                    type_bridge_row_value_long(row, view("long"), &mut long),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(long, -7);
                let mut bits = 0;
                assert_eq!(
                    type_bridge_row_value_double(row, view("double"), &mut bits),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(f64::from_bits(bits), 1.25);
                let mut boolean = 0;
                assert_eq!(
                    type_bridge_row_value_boolean(row, view("boolean"), &mut boolean),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(boolean, 1);
                type TextGetter = unsafe extern "C" fn(
                    *const TypeBridgeRow,
                    TypeBridgeByteView,
                    *mut TypeBridgeByteView,
                ) -> TypeBridgeStatus;
                for (name, getter, expected) in [
                    (
                        "date",
                        type_bridge_row_value_date as TextGetter,
                        "2026-10-26",
                    ),
                    (
                        "datetime",
                        type_bridge_row_value_datetime,
                        "2026-10-26T01:02:03",
                    ),
                    (
                        "datetime_tz",
                        type_bridge_row_value_datetime_tz,
                        "2026-10-26T01:02:03Z",
                    ),
                    ("decimal", type_bridge_row_value_decimal, "12.34"),
                    ("duration", type_bridge_row_value_duration, "P1D"),
                ] {
                    assert_eq!(
                        getter(row, view(name), &mut text_value),
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(text(text_value), expected);
                }
                assert_eq!(
                    type_bridge_row_value_date(row, view("date"), &mut text_value),
                    TypeBridgeStatus::Ok
                );
                let before = text(text_value);
                assert_eq!(
                    type_bridge_document_attribute_kind(
                        std::ptr::from_ref(row).cast(),
                        view("date"),
                        0,
                        text_value.data.cast_mut().cast()
                    ),
                    INVALID
                );
                assert_eq!(
                    text(text_value),
                    before,
                    "foreign document getters must fence row bytes"
                );
                let mut legacy_value = 99;
                assert_eq!(
                    crate::staged::type_bridge_pending_answer(
                        std::ptr::from_ref(row).cast(),
                        text_value.data.cast_mut().cast(),
                        &mut legacy_value
                    ),
                    INVALID
                );
                assert_eq!(text(text_value), before);
                assert_eq!(legacy_value, 99);
                assert_eq!(
                    type_bridge_pending_document_count(
                        std::ptr::from_ref(row).cast(),
                        text_value.data.cast_mut().cast()
                    ),
                    INVALID
                );
                assert_eq!(
                    text(text_value),
                    before,
                    "foreign pending getters must fence row bytes"
                );
                let mut kind = 0;
                assert_eq!(
                    type_bridge_row_value_kind(row, view("long"), &mut kind),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(kind, TypeBridgeValueKind::Long as u32);
                assert_eq!(
                    type_bridge_row_value_kind(row, view("absent"), &mut kind),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(kind, TypeBridgeValueKind::Absent as u32);
                assert_eq!(
                    type_bridge_row_value_string(row, view("absent"), &mut text_value),
                    INVALID
                );

                assert_eq!(
                    type_bridge_row_value_long(row, view("text"), &mut long),
                    INVALID
                );
                assert_eq!(
                    type_bridge_row_value_kind(row, view("unknown"), &mut kind),
                    INVALID
                );
                assert_eq!(
                    type_bridge_document_iid(std::ptr::from_ref(row).cast(), &mut text_value),
                    INVALID
                );
                assert_eq!(
                    type_bridge_row_value_boolean(
                        row,
                        view("boolean"),
                        std::ptr::from_ref(&rows[1]).cast_mut().cast()
                    ),
                    INVALID
                );
            }
        }
    }
}
