//! Verified package-scoped runtime projections for generated Python models.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pyo3::prelude::*;
#[cfg(test)]
use pyo3::types::PyWeakrefMethods;
use pyo3::types::{
    PyAny, PyBool, PyBytes, PyCFunction, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType,
    PyWeakrefReference,
};
use pythonize::pythonize;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::decimal::parse_decimal;
use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projected_record::{
    MAX_PROJECTED_ARCHIVE_BYTES, MAX_PROJECTED_ARCHIVE_RECORDS, MAX_PROJECTED_DECODED_WEIGHT,
};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedContainer, ProjectedModelForm, ProjectedModelUse,
    ProjectedMultiplicity, ProjectedTokenIdentity, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::DeclaredIdentityFingerprint;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProjectionEvidenceSlotPresence,
    SdkQueryDiagnosticPathKind,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, CanonicalTime,
    TimeZoneDesignator,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_core_lib::ast::{Clause, Constraint, Pattern, RolePlayer, Statement};
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use type_bridge_orm::_dynamic::{
    DynamicAttributeMap, DynamicComparisonOp, DynamicEntityRow, DynamicExpr, DynamicRelationRow,
    DynamicRolePlayer, DynamicRolePlayerInput,
};
use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::projected_batch::ProjectedBatchBindingBudget;
use type_bridge_orm::session::{Database, TransactionContext, TransactionContextState};
use type_bridge_orm::value::AttributeValue;
use type_bridge_orm::{
    AnswerCancellation, HydratedAttribute, HydratedRolePlayer, HydratedThing,
    ProjectedAttributeValue, ProjectedBatch, ProjectedBatchExecutor,
    ProjectedBatchInvocationControl, ProjectedBatchOperation, ProjectedBatchResult,
    ProjectedBatchRow, ProjectedCodecValue, ProjectedCreate, ProjectedCreateBudget,
    ProjectedCrudExecutor, ProjectedManagerComparison, ProjectedManagerFilter,
    ProjectedManagerFilterExecutor, ProjectedReference, ProjectedReferenceOrigin,
    ProjectedRolePlayer, ProjectedThing, QueryExecutionResourceLimits, ThingKind,
    resolve_generated_manager_lookup,
};
use type_bridge_orm::{InstalledRuntimeProjection, ProviderRuntimeOwner};
use type_bridge_schema::{decode_schema_authority, schema_authority_capability_vocabulary};
use type_bridge_schema_codegen::{PythonEmitter, verify_projection_evidence};

use crate::match_runtime::{
    PyMatchSessionHandle, PyQueryCancellation, PyQueryExecutionResourceLimits, py_sdk_diagnostic,
};
use crate::orm_runtime::{
    PyRustDatabase, PyRustTransactionContext, provider_block_on, provider_block_on_with_gil,
};
use crate::validated_result_runtime::PyValidatedMatchThingHandle;

struct PythonCanonicalControl {
    cancellation: AnswerCancellation,
    deadline: Option<Instant>,
    max_input_bytes: usize,
    max_output_bytes: usize,
    max_depth: usize,
    max_records: usize,
    max_members: usize,
}

impl PythonCanonicalControl {
    fn capture(
        cancellation: Option<&PyQueryCancellation>,
        timeout_milliseconds: Option<u64>,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
        max_depth: Option<usize>,
        max_records: Option<usize>,
        max_members: Option<usize>,
    ) -> PyResult<Self> {
        let deadline = timeout_milliseconds
            .map(|milliseconds| {
                Instant::now()
                    .checked_add(Duration::from_millis(milliseconds))
                    .ok_or_else(|| {
                        py_sdk_diagnostic(
                            SdkExecutionDiagnostic::projected_codec_deadline_exceeded(),
                        )
                    })
            })
            .transpose()?;
        Ok(Self {
            cancellation: cancellation
                .map_or_else(AnswerCancellation::default, |value| value.inner()),
            deadline,
            max_input_bytes: max_input_bytes
                .unwrap_or(MAX_PROJECTED_ARCHIVE_BYTES)
                .min(MAX_PROJECTED_ARCHIVE_BYTES),
            max_output_bytes: max_output_bytes
                .unwrap_or(MAX_PROJECTED_ARCHIVE_BYTES)
                .min(MAX_PROJECTED_ARCHIVE_BYTES),
            max_depth: max_depth
                .unwrap_or(MAX_CANONICAL_DEPTH)
                .min(MAX_CANONICAL_DEPTH),
            max_records: max_records
                .unwrap_or(MAX_PROJECTED_ARCHIVE_RECORDS)
                .min(MAX_PROJECTED_ARCHIVE_RECORDS),
            max_members: max_members
                .unwrap_or(MAX_PROJECTED_DECODED_WEIGHT)
                .min(MAX_PROJECTED_DECODED_WEIGHT)
                .min(MAX_CANONICAL_COLLECTION_LEN),
        })
    }

    fn check(&self) -> PyResult<()> {
        if self.cancellation.is_cancelled() {
            Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_cancelled(),
            ))
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_deadline_exceeded(),
            ))
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

fn python_canonical_limit_error(
    error: type_bridge_contract::diagnostic::Diagnostic,
    input: bool,
) -> PyErr {
    let diagnostic = match error.code().as_str() {
        "canonical_json_too_deep" => SdkExecutionDiagnostic::projected_codec_depth_limit(),
        "canonical_collection_too_large" => SdkExecutionDiagnostic::projected_codec_member_limit(),
        "canonical_json_too_large" | "canonical_string_too_large" if input => {
            SdkExecutionDiagnostic::projected_codec_input_limit()
        }
        "canonical_json_too_large" | "canonical_string_too_large" => {
            SdkExecutionDiagnostic::projected_codec_output_limit()
        }
        _ => return py_diagnostic(error),
    };
    py_sdk_diagnostic(diagnostic)
}

fn python_canonical_codec_error(error: type_bridge_orm::ProjectedCodecError) -> PyErr {
    if matches!(
        &error,
        type_bridge_orm::ProjectedCodecError::Contract(diagnostic)
            if diagnostic.code().as_str() == "projected_codec_declared_schema_mismatch"
    ) {
        return py_sdk_diagnostic(SdkExecutionDiagnostic::projected_record_schema_mismatch());
    }
    py_value_error(error.to_string())
}

struct RegisteredModel {
    complete: Py<PyType>,
    reference: Option<Py<PyType>>,
    batch_slots: Option<Arc<ProjectedFacadeSlots>>,
    batch_reference_slots: Option<Arc<ProjectedFacadeSlots>>,
}

struct InstalledPackage {
    projection: Arc<InstalledRuntimeProjection>,
    managed_scope_id: Option<ManagedScopeId>,
    models: BTreeMap<TypeId, RegisteredModel>,
    structs: BTreeMap<TypeId, Py<PyType>>,
    types_by_label: BTreeMap<String, TypeId>,
    facade_origins: FacadeOriginRegistry,
    named_zone_marker: Option<Py<PyType>>,
    named_zone_slots: Option<ProjectedNamedZoneSlots>,
    scalar_hydration: Option<ProjectedScalarHydration>,
}

struct ProjectedScalarHydration {
    date_type: Py<PyType>,
    datetime_type: Py<PyType>,
    datetime_replace: Py<PyAny>,
    tzinfo_key: Py<PyAny>,
    timedelta_type: Py<PyType>,
    timezone_type: Py<PyType>,
    zoneinfo_type: Py<PyType>,
    decimal_type: Py<PyType>,
}

struct ProjectedNamedZoneSlots {
    class: Py<PyType>,
    offset: ProjectedFacadeSlot,
    zone: ProjectedFacadeSlot,
    heap_allocator: Arc<ProjectedHeapAllocator>,
    allocator: ProjectedNamedZoneAllocator,
    trusted_mro: Vec<ProjectedTypeLayout>,
}

struct ProjectedHeapAllocator {
    object_new: Py<PyAny>,
    trusted_deallocator: usize,
    trusted_constructor: usize,
    trusted_free: usize,
    trusted_traverse: usize,
    trusted_clear: usize,
    trusted_is_gc: usize,
    trusted_gc_flags: std::ffi::c_ulong,
}

struct ProjectedNamedZoneAllocator {
    constructor: Py<PyAny>,
    trusted_constructor: usize,
    base: Py<PyType>,
}

struct ProjectedTypeLayout {
    class: Py<PyType>,
    flags: std::ffi::c_ulong,
    basicsize: isize,
    itemsize: isize,
    dictoffset: isize,
    weakrefoffset: isize,
}

struct FacadeOriginEntry {
    facade: Py<PyWeakrefReference>,
    active: Option<FacadeProjectionProof>,
    pending: Option<PendingFacadeProof>,
    retired: Option<ProjectedFacadeSnapshot>,
}

struct PendingFacadeProof {
    token: u64,
    proof: FacadeProjectionProof,
    retired: Option<ProjectedFacadeSnapshot>,
}

#[derive(Clone, Copy)]
struct PendingFacadeActivation {
    pointer: usize,
    token: u64,
}

struct PreparedFacadeOrigin {
    pointer: usize,
    facade: Py<PyWeakrefReference>,
}

impl PreparedFacadeOrigin {
    fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            pointer: self.pointer,
            facade: self.facade.clone_ref(py),
        }
    }
}

struct ProjectedFacadeSnapshot {
    iid: Py<PyAny>,
    values: Py<PyAny>,
}

impl ProjectedFacadeSnapshot {
    fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            iid: self.iid.clone_ref(py),
            values: self.values.clone_ref(py),
        }
    }
}

struct PreparedBatchFacade {
    instance: Py<PyAny>,
    origin: PreparedFacadeOrigin,
    snapshot: ProjectedFacadeSnapshot,
}

struct StagedBatchFacade {
    instance: Py<PyAny>,
    snapshot: ProjectedFacadeSnapshot,
}

struct PendingFacadeOrigin {
    origin: PreparedFacadeOrigin,
    proof: FacadeProjectionProof,
}

struct StagedBatchHydration {
    projected: Arc<ProjectedThing>,
    _instance: Py<PyAny>,
    replacement: ProjectedFacadeSnapshot,
}

struct PendingActivationGuard {
    registry: FacadeOriginRegistry,
    activations: Vec<PendingFacadeActivation>,
    armed: bool,
}

struct BatchHydrationPoolState {
    objects: Vec<Py<PyAny>>,
    overflow: Option<Py<PyAny>>,
    identities: HashSet<usize>,
    limit: usize,
    reserved: bool,
}

struct BatchHydrationPool {
    state: Mutex<BatchHydrationPoolState>,
}

impl PendingActivationGuard {
    fn new(
        registry: FacadeOriginRegistry,
        capacity: usize,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Ok(Self {
            registry,
            activations: reserved_binding_rows(capacity)?,
            armed: true,
        })
    }

    fn stage(
        &mut self,
        py: Python<'_>,
        origin: PreparedFacadeOrigin,
        proof: FacadeProjectionProof,
        retired: Option<ProjectedFacadeSnapshot>,
    ) -> PyResult<()> {
        let activation = self.registry.stage(py, origin, proof, retired)?;
        self.activations.push(activation);
        Ok(())
    }

    fn into_activations(mut self) -> Vec<PendingFacadeActivation> {
        self.armed = false;
        std::mem::take(&mut self.activations)
    }
}

impl BatchHydrationPool {
    fn try_new(facades: &[StagedBatchFacade]) -> Result<Self, SdkExecutionDiagnostic> {
        let mut identities = HashSet::new();
        identities
            .try_reserve(facades.len())
            .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
        for facade in facades {
            identities.insert(facade.instance.as_ptr() as usize);
        }
        Ok(Self {
            state: Mutex::new(BatchHydrationPoolState {
                objects: Vec::new(),
                overflow: None,
                identities,
                limit: 0,
                reserved: false,
            }),
        })
    }

    fn reserve_exact(&self, count: usize) -> Result<(), SdkExecutionDiagnostic> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.reserved {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        state
            .objects
            .try_reserve_exact(count)
            .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
        state
            .identities
            .try_reserve(count)
            .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
        state.limit = count;
        state.reserved = true;
        Ok(())
    }

    fn retain_fresh(&self, value: &Bound<'_, PyAny>, expected: &Bound<'_, PyType>) -> PyResult<()> {
        let pointer = value.as_ptr() as usize;
        // SAFETY: both objects are live under the GIL. Raw type identity does
        // not create an owned Bound whose DECREF could occur under the pool
        // mutex.
        let exact = unsafe { pyo3::ffi::Py_TYPE(value.as_ptr()) } == expected.as_ptr().cast();
        let retained = value.clone().unbind();
        let unique = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.reserved || state.objects.len() >= state.limit {
                if state.overflow.is_some() {
                    fatal_batch_invariant(pyo3::ffi::c_str!(
                        "projected hydration retained more than one overflow object"
                    ));
                }
                state.overflow = Some(retained);
                None
            } else {
                // Retain before validation: a hostile wrong-class or aliased
                // result cannot deallocate until common rollback/poison and
                // lease cleanup have completed on the outer worker frame.
                state.objects.push(retained);
                Some(state.identities.insert(pointer))
            }
        };
        let Some(unique) = unique else {
            return Err(py_runtime_error(
                "projected hydration exceeded its prevalidated object forecast",
            ));
        };
        if !exact {
            return Err(py_runtime_error(
                "trusted successor allocator returned the wrong exact class",
            ));
        }
        if !unique {
            return Err(py_runtime_error(
                "trusted successor allocator returned an input or repeated object identity",
            ));
        }
        Ok(())
    }

    fn verify_fully_consumed(&self) -> Result<(), SdkExecutionDiagnostic> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.reserved || state.objects.len() != state.limit || state.overflow.is_some() {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        Ok(())
    }
}

impl Drop for PendingActivationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.registry.abort(&self.activations);
        }
    }
}

struct BatchPublicationGuard<'py> {
    py: Python<'py>,
    package: Arc<InstalledPackage>,
    slots: Arc<ProjectedFacadeSlots>,
    facades: Vec<PreparedBatchFacade>,
    _hydrated: Vec<StagedBatchHydration>,
    output: Option<Py<PyAny>>,
    activations: Vec<PendingFacadeActivation>,
    published: usize,
    armed: bool,
}

struct SuccessorBatchGcGuard<'py> {
    _py: Python<'py>,
    restore_enabled: bool,
}

impl<'py> SuccessorBatchGcGuard<'py> {
    fn disable(py: Python<'py>) -> Self {
        // SAFETY: called only on the dedicated worker while it owns the GIL.
        let restore_enabled = unsafe { pyo3::ffi::PyGC_Disable() } != 0;
        Self {
            _py: py,
            restore_enabled,
        }
    }
}

impl Drop for SuccessorBatchGcGuard<'_> {
    fn drop(&mut self) {
        if self.restore_enabled {
            // SAFETY: the guard is destroyed on the same worker under the GIL,
            // after mapped executor cleanup and publication/rollback finish.
            unsafe {
                pyo3::ffi::PyGC_Enable();
            }
        } else {
            // Restore an already-disabled caller state even if interior code
            // accidentally enabled collection while the guard was active.
            unsafe {
                pyo3::ffi::PyGC_Disable();
            }
        }
    }
}

enum BatchMappedOutput<'py> {
    Empty(Py<PyAny>),
    Publication(BatchPublicationGuard<'py>),
}

impl BatchMappedOutput<'_> {
    fn finish(self) -> Py<PyAny> {
        match self {
            Self::Empty(output) => output,
            Self::Publication(guard) => guard.finish(),
        }
    }
}

impl BatchPublicationGuard<'_> {
    fn finish(mut self) -> Py<PyAny> {
        self.package.facade_origins.activate(&self.activations);
        self.armed = false;
        self.output
            .take()
            .expect("a successful nonempty mapped batch retains its output list")
    }
}

impl Drop for BatchPublicationGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for facade in self.facades.iter().take(self.published) {
            self.slots
                .replace_infallible(self.py, facade.instance.bind(self.py), &facade.snapshot);
        }
        self.package.facade_origins.abort(&self.activations);
    }
}

struct ProjectedFacadeSlot {
    name: &'static str,
    descriptor: Py<PyAny>,
    owner: Py<PyType>,
    getter: pyo3::ffi::descrgetfunc,
    setter: pyo3::ffi::descrsetfunc,
}

struct ProjectedFacadeSlots {
    class: Py<PyType>,
    values: ProjectedFacadeSlot,
    iid: ProjectedFacadeSlot,
    attribute_value: Option<ProjectedFacadeSlot>,
    allocator: Arc<ProjectedHeapAllocator>,
    trusted_mro: Vec<ProjectedTypeLayout>,
}

#[derive(Clone)]
enum FacadeProjectionProof {
    Thing(Arc<ProjectedThing>),
    DetachedSnapshot,
    Reference(Arc<ProjectedReference>),
    RolePlayer {
        parent: Arc<ProjectedThing>,
        role_ordinal: usize,
        player_ordinal: usize,
    },
}

impl FacadeProjectionProof {
    fn retained_role_player(
        parent: &ProjectedThing,
        role_ordinal: usize,
        player_ordinal: usize,
    ) -> Result<&ProjectedRolePlayer, SdkExecutionDiagnostic> {
        parent
            .roles()
            .values()
            .nth(role_ordinal)
            .and_then(|players| players.get(player_ordinal))
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }

    fn reference(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
        match self {
            Self::Thing(thing) => thing.try_to_reference(installed),
            Self::DetachedSnapshot => Err(SdkExecutionDiagnostic::projected_snapshot_detached()),
            Self::Reference(reference) => Ok(reference.as_ref().clone()),
            Self::RolePlayer {
                parent,
                role_ordinal,
                player_ordinal,
            } => {
                let player =
                    Self::retained_role_player(parent.as_ref(), *role_ordinal, *player_ordinal)?;
                player.validate_for(installed)?;
                Ok(player.reference().clone())
            }
        }
    }

    fn origin_carrier(&self) -> Result<Option<ProjectedReferenceOrigin>, SdkExecutionDiagnostic> {
        match self {
            Self::Thing(thing) => Ok(thing.origin_carrier()),
            Self::DetachedSnapshot => Err(SdkExecutionDiagnostic::projected_snapshot_detached()),
            Self::Reference(reference) => Ok(reference.origin_carrier()),
            Self::RolePlayer {
                parent,
                role_ordinal,
                player_ordinal,
            } => Ok(
                Self::retained_role_player(parent.as_ref(), *role_ordinal, *player_ordinal)?
                    .reference()
                    .origin_carrier(),
            ),
        }
    }
}

#[derive(Clone)]
struct FacadeOriginRegistry {
    entries: Arc<Mutex<HashMap<usize, FacadeOriginEntry>>>,
    next_pending_token: Arc<AtomicU64>,
    callback: Arc<Py<PyAny>>,
}

fn retained_weakref_target(
    py: Python<'_>,
    reference: &Bound<'_, PyWeakrefReference>,
) -> Option<Py<PyAny>> {
    let mut target = std::ptr::null_mut();
    // SAFETY: the registry stores only exact live weakref.ReferenceType
    // objects. PyWeakref_GetRef is Stable ABI and returns one owned target
    // reference when the referent remains live.
    match unsafe { pyo3::ffi::compat::PyWeakref_GetRef(reference.as_ptr(), &mut target) } {
        0 => None,
        1..=std::os::raw::c_int::MAX => {
            // SAFETY: the successful call returned one owned reference.
            Some(unsafe { Bound::<PyAny>::from_owned_ptr(py, target).unbind() })
        }
        _ => fatal_batch_invariant(pyo3::ffi::c_str!(
            "projected facade registry retained an invalid weak reference"
        )),
    }
}

impl FacadeOriginRegistry {
    fn new(py: Python<'_>) -> PyResult<Self> {
        let entries = Arc::new(Mutex::new(HashMap::<usize, FacadeOriginEntry>::new()));
        let callback_entries = Arc::downgrade(&entries);
        let callback =
            PyCFunction::new_closure(py, None, None, move |args, _kwargs| -> PyResult<()> {
                let Some(entries) = callback_entries.upgrade() else {
                    return Ok(());
                };
                let expired = args.get_item(0)?;
                let expired = expired.cast::<PyWeakrefReference>()?;
                if let Some(live) = retained_weakref_target(expired.py(), expired) {
                    // A caller can obtain and invoke the shared callback. A
                    // live weakref is never eligible for registry cleanup.
                    drop(live);
                    return Ok(());
                }
                let removed = {
                    let mut entries = entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let pointer = entries.iter().find_map(|(pointer, entry)| {
                        entry
                            .facade
                            .bind(expired.py())
                            .is(expired)
                            .then_some(*pointer)
                    });
                    pointer.and_then(|pointer| entries.remove(&pointer))
                };
                // Dropping retired Python referents may run finalizers. Never
                // hold the origin-registry mutex across their decrefs.
                drop(removed);
                Ok(())
            })?;
        Ok(Self {
            entries,
            next_pending_token: Arc::new(AtomicU64::new(1)),
            callback: Arc::new(callback.into_any().unbind()),
        })
    }

    #[cfg(test)]
    fn retain_reference(
        &self,
        value: &Bound<'_, PyAny>,
        reference: Arc<ProjectedReference>,
    ) -> PyResult<()> {
        let prepared = self.prepare(value)?;
        self.install(prepared, FacadeProjectionProof::Reference(reference));
        Ok(())
    }

    fn prepare(&self, value: &Bound<'_, PyAny>) -> PyResult<PreparedFacadeOrigin> {
        let pointer = value.as_ptr() as usize;
        let py = value.py();
        let facade = PyWeakrefReference::new_with(value, self.callback.bind(py))?.unbind();
        Ok(PreparedFacadeOrigin { pointer, facade })
    }

    fn install(&self, prepared: PreparedFacadeOrigin, proof: FacadeProjectionProof) {
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut removed = entries.remove(&prepared.pointer);
            let retired = removed.as_mut().and_then(|entry| entry.retired.take());
            entries.insert(
                prepared.pointer,
                FacadeOriginEntry {
                    facade: prepared.facade,
                    active: Some(proof),
                    pending: None,
                    retired,
                },
            );
            removed
        };
        // The removed weakref/pending state may own Python finalizers.
        drop(removed);
    }

    fn stage(
        &self,
        py: Python<'_>,
        prepared: PreparedFacadeOrigin,
        proof: FacadeProjectionProof,
        retired: Option<ProjectedFacadeSnapshot>,
    ) -> PyResult<PendingFacadeActivation> {
        let token = self.next_pending_token.fetch_add(1, Ordering::Relaxed);
        if token == 0 {
            return Err(py_runtime_error(
                "projected facade pending-origin token space was exhausted",
            ));
        }
        let activation = PendingFacadeActivation {
            pointer: prepared.pointer,
            token,
        };
        let (status, upgraded, removed) = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let upgraded = entries
                .get(&prepared.pointer)
                .and_then(|entry| retained_weakref_target(py, entry.facade.bind(py)));
            let same_facade = upgraded
                .as_ref()
                .is_some_and(|facade| facade.as_ptr() as usize == prepared.pointer);
            if same_facade {
                let entry = entries
                    .get_mut(&prepared.pointer)
                    .expect("the matching facade entry remains present while locked");
                let status = if entry.pending.is_some() {
                    1
                } else if entry.retired.is_some() && retired.is_some() {
                    2
                } else {
                    entry.pending = Some(PendingFacadeProof {
                        token,
                        proof,
                        retired,
                    });
                    0
                };
                (status, upgraded, None)
            } else if entries.try_reserve(1).is_err() {
                (3, upgraded, None)
            } else {
                let removed = entries.remove(&prepared.pointer);
                entries.insert(
                    prepared.pointer,
                    FacadeOriginEntry {
                        facade: prepared.facade,
                        active: None,
                        pending: Some(PendingFacadeProof {
                            token,
                            proof,
                            retired,
                        }),
                        retired: None,
                    },
                );
                (0, upgraded, removed)
            }
        };
        // Both the upgraded facade and any replaced entry own Python refs.
        // Release them only after the origin-registry mutex is unlocked.
        drop(upgraded);
        drop(removed);
        match status {
            0 => Ok(activation),
            1 => Err(py_runtime_error(
                "projected facade already has a pending origin proof",
            )),
            2 => Err(py_runtime_error(
                "projected facade retained slots were not drained before staging",
            )),
            _ => Err(py_sdk_diagnostic(
                ProjectedBatch::binding_allocation_failure(),
            )),
        }
    }

    fn activate(&self, activations: &[PendingFacadeActivation]) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for activation in activations {
            let Some(entry) = entries.get_mut(&activation.pointer) else {
                fatal_batch_invariant(pyo3::ffi::c_str!(
                    "pending projected facade origin disappeared"
                ));
            };
            let Some(pending) = entry.pending.take() else {
                fatal_batch_invariant(pyo3::ffi::c_str!(
                    "pending projected facade origin was not staged"
                ));
            };
            if pending.token != activation.token {
                fatal_batch_invariant(pyo3::ffi::c_str!(
                    "pending projected facade origin token changed"
                ));
            }
            entry.active = Some(pending.proof);
            if let Some(retired) = pending.retired {
                if entry.retired.is_some() {
                    fatal_batch_invariant(pyo3::ffi::c_str!(
                        "projected facade retained slots were replaced before draining"
                    ));
                }
                entry.retired = Some(retired);
            }
        }
    }

    fn abort(&self, activations: &[PendingFacadeActivation]) {
        for activation in activations {
            let (pending, removed) = {
                let mut entries = self
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let pending = entries.get_mut(&activation.pointer).and_then(|entry| {
                    entry
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.token == activation.token)
                        .then(|| entry.pending.take())
                        .flatten()
                });
                let remove = entries
                    .get(&activation.pointer)
                    .is_some_and(|entry| entry.active.is_none() && entry.pending.is_none());
                let removed = remove
                    .then(|| entries.remove(&activation.pointer))
                    .flatten();
                (pending, removed)
            };
            // A retired snapshot may contain the last reference to hostile
            // generated values. Never decref it while the mutex is held.
            drop(pending);
            drop(removed);
        }
    }

    fn proof(&self, value: &Bound<'_, PyAny>) -> Option<FacadeProjectionProof> {
        let pointer = value.as_ptr() as usize;
        let py = value.py();
        let (retained, upgraded, removed) = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let upgraded = entries
                .get(&pointer)
                .and_then(|entry| retained_weakref_target(py, entry.facade.bind(py)));
            let live = upgraded
                .as_ref()
                .is_some_and(|facade| facade.bind(py).is(value));
            let retained = live
                .then(|| entries.get(&pointer).and_then(|entry| entry.active.clone()))
                .flatten();
            let removed = (!live).then(|| entries.remove(&pointer)).flatten();
            (retained, upgraded, removed)
        };
        drop(upgraded);
        drop(removed);
        retained
    }

    fn take_retired(&self, value: &Bound<'_, PyAny>) -> Option<ProjectedFacadeSnapshot> {
        let pointer = value.as_ptr() as usize;
        let py = value.py();
        let (retired, upgraded, removed) = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let upgraded = entries
                .get(&pointer)
                .and_then(|entry| retained_weakref_target(py, entry.facade.bind(py)));
            let live = upgraded
                .as_ref()
                .is_some_and(|facade| facade.bind(py).is(value));
            if live {
                (
                    entries
                        .get_mut(&pointer)
                        .and_then(|entry| entry.retired.take()),
                    upgraded,
                    None,
                )
            } else {
                (None, upgraded, entries.remove(&pointer))
            }
        };
        drop(upgraded);
        drop(removed);
        retired
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|entry| entry.active.is_some())
            .count()
    }
}

impl InstalledPackage {
    fn model_id_for_class(&self, py: Python<'_>, class: &Py<PyType>) -> PyResult<TypeId> {
        let pointer = class.bind(py).as_ptr();
        self.models
            .iter()
            .find_map(|(id, registered)| {
                (registered.complete.bind(py).as_ptr() == pointer).then(|| id.clone())
            })
            .ok_or_else(|| {
                py_type_error("model class is not registered in this runtime projection")
            })
    }

    fn model_id_for_reference_class(&self, py: Python<'_>, class: &Py<PyType>) -> PyResult<TypeId> {
        let pointer = class.bind(py).as_ptr();
        self.models
            .iter()
            .find_map(|(id, registered)| {
                registered
                    .reference
                    .as_ref()
                    .is_some_and(|reference| reference.bind(py).as_ptr() == pointer)
                    .then(|| id.clone())
            })
            .ok_or_else(|| {
                py_type_error("reference class is not registered in this runtime projection")
            })
    }

    fn struct_id_for_class(&self, py: Python<'_>, class: &Py<PyType>) -> PyResult<TypeId> {
        let pointer = class.bind(py).as_ptr();
        self.structs
            .iter()
            .find_map(|(id, registered)| {
                (registered.bind(py).as_ptr() == pointer).then(|| id.clone())
            })
            .ok_or_else(|| {
                py_type_error("struct class is not registered in this runtime projection")
            })
    }

    fn identify_value(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<(TypeId, ProjectedModelForm)> {
        let pointer = value.get_type().as_ptr();
        for (id, registered) in &self.models {
            if registered.complete.bind(py).as_ptr() == pointer {
                return Ok((id.clone(), ProjectedModelForm::Complete));
            }
            if registered
                .reference
                .as_ref()
                .is_some_and(|reference| reference.bind(py).as_ptr() == pointer)
            {
                return Ok((id.clone(), ProjectedModelForm::Reference));
            }
        }
        Err(py_type_error(
            "projected value is not an exact registered complete or reference class",
        ))
    }

    fn class(&self, id: &TypeId, form: ProjectedModelForm) -> PyResult<&Py<PyType>> {
        let registered = self
            .models
            .get(id)
            .ok_or_else(|| py_runtime_error("projection model class is not installed"))?;
        match form {
            ProjectedModelForm::Complete => Ok(&registered.complete),
            ProjectedModelForm::Reference => registered.reference.as_ref().ok_or_else(|| {
                py_runtime_error("projection requested an unregistered reference class")
            }),
        }
    }

    fn batch_slots(&self, id: &TypeId) -> PyResult<Arc<ProjectedFacadeSlots>> {
        self.models
            .get(id)
            .and_then(|registered| registered.batch_slots.as_ref())
            .cloned()
            .ok_or_else(|| {
                py_runtime_error("successor batch facade slots were not installed for the model")
            })
    }

    fn batch_slots_for(
        &self,
        id: &TypeId,
        form: ProjectedModelForm,
    ) -> PyResult<Arc<ProjectedFacadeSlots>> {
        let registered = self
            .models
            .get(id)
            .ok_or_else(|| py_runtime_error("projection model class is not installed"))?;
        match form {
            ProjectedModelForm::Complete => registered.batch_slots.as_ref(),
            ProjectedModelForm::Reference => registered.batch_reference_slots.as_ref(),
        }
        .cloned()
        .ok_or_else(|| {
            py_runtime_error("successor hydration slots were not installed for the model form")
        })
    }

    fn validate_batch_hydration_layouts(&self, py: Python<'_>) -> PyResult<()> {
        for registered in self.models.values() {
            if let Some(slots) = &registered.batch_slots {
                slots.validate_layout(py)?;
            }
            if let Some(slots) = &registered.batch_reference_slots {
                slots.validate_layout(py)?;
            }
        }
        if let Some(slots) = &self.named_zone_slots {
            slots.validate_layout(py)?;
        }
        Ok(())
    }

    fn type_by_label(&self, label: &str, kind: TypeKind) -> PyResult<&TypeId> {
        self.types_by_label
            .get(label)
            .filter(|id| id.kind() == kind)
            .ok_or_else(|| py_runtime_error(format!("projection has no {kind:?} type {label:?}")))
    }
}

fn direct_tls_from_python(
    tls_mode: &str,
    tls_root_ca: Option<std::path::PathBuf>,
) -> PyResult<type_bridge_orm::DirectTls> {
    match (tls_mode, tls_root_ca) {
        ("disabled", None) => Ok(type_bridge_orm::DirectTls::disabled()),
        ("native_roots", None) => Ok(type_bridge_orm::DirectTls::native_roots()),
        ("custom_root", Some(path)) => {
            type_bridge_orm::DirectTls::custom_root(path).map_err(py_sdk_diagnostic)
        }
        ("custom_root", None) => Err(pyo3::exceptions::PyValueError::new_err(
            "custom_root TLS requires tls_root_ca",
        )),
        ("disabled" | "native_roots", Some(_)) => Err(pyo3::exceptions::PyValueError::new_err(
            "tls_root_ca is valid only with custom_root TLS",
        )),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            "tls_mode must be disabled, native_roots, or custom_root",
        )),
    }
}

/// A verified runtime projection installed for exactly one generated package.
#[pyclass]
pub struct PyRuntimeProjection {
    package: Arc<InstalledPackage>,
}

#[pymethods]
impl PyRuntimeProjection {
    /// Verify canonical projection bytes and install their exact generated classes.
    #[new]
    #[pyo3(signature = (
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
        schema_authority = None,
        structs = Vec::new(),
    ))]
    fn new(
        py: Python<'_>,
        projection_json: &str,
        semantic_fingerprint_json: &str,
        projection_fingerprint_json: &str,
        models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
        schema_authority: Option<&Bound<'_, PyBytes>>,
        structs: Vec<Py<PyType>>,
    ) -> PyResult<Self> {
        install_projection_with_structs(
            py,
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            models,
            structs,
            schema_authority.map(|authority| authority.as_bytes()),
        )
        .map(|package| Self { package })
    }

    /// Open a database through this installed package's verified authority.
    #[pyo3(signature = (
        endpoint,
        database,
        username="admin",
        password="password",
        http_port=type_bridge_core_lib::version::DEFAULT_HTTP_PORT,
        tls_mode="disabled",
        tls_root_ca=None,
        connection_limits=None,
        answer_limits=None,
        cancellation=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn connect_direct(
        &self,
        py: Python<'_>,
        endpoint: String,
        database: String,
        username: &str,
        password: &str,
        http_port: u16,
        tls_mode: &str,
        tls_root_ca: Option<std::path::PathBuf>,
        connection_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        answer_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        cancellation: Option<PyRef<'_, PyQueryCancellation>>,
    ) -> PyResult<PyRustDatabase> {
        let tls = direct_tls_from_python(tls_mode, tls_root_ca)?;
        let mut policy =
            type_bridge_orm::DirectConnectionPolicy::new(endpoint, database, username, password)
                .http_port(http_port)
                .tls(tls);
        if let Some(limits) = connection_limits {
            policy = policy.connection_limits(limits.inner());
        }
        if let Some(limits) = answer_limits {
            policy = policy.answer_limits(limits.inner());
        }
        let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "Failed to create Tokio runtime: {error}"
            ))
        })?;
        let cancellation = cancellation.map(|value| value.inner()).unwrap_or_default();
        let database = provider_block_on(
            py,
            runtime.as_ref(),
            Database::connect_direct(self.package.projection.as_ref(), &policy, cancellation),
        )
        .map_err(py_sdk_diagnostic)?;
        Ok(PyRustDatabase::from_handles(
            Arc::new(database),
            runtime,
            self.package.managed_scope_id.clone(),
        ))
    }

    /// Bind an exact generated model class to an existing Rust database handle.
    fn manager_for_database(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        database: &PyRustDatabase,
    ) -> PyResult<PyProjectedModelManager> {
        let type_id = self.package.model_id_for_class(py, &model)?;
        ensure_manageable(self.package.as_ref(), &type_id)?;
        let compatibility_filter = initial_compatibility_filter(self.package.as_ref(), &type_id)?;
        let (database, runtime) = database.handles();
        Ok(PyProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: Some(database),
            transaction: None,
            successor_batch_marker: None,
            runtime,
            filters: vec![],
            compatibility_filter,
        })
    }

    /// Bind an exact generated model class to an existing Rust transaction handle.
    fn manager_for_transaction(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        transaction: &PyRustTransactionContext,
    ) -> PyResult<PyProjectedModelManager> {
        let type_id = self.package.model_id_for_class(py, &model)?;
        ensure_manageable(self.package.as_ref(), &type_id)?;
        let successor_batch_marker = transaction.successor_batch_marker();
        let (transaction, runtime) = transaction.handles();
        let compatibility_filter = if transaction.tx_type() == type_bridge_orm::TxType::Read {
            initial_compatibility_filter(self.package.as_ref(), &type_id)?
        } else {
            None
        };
        Ok(PyProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: None,
            transaction: Some(transaction),
            successor_batch_marker: Some(successor_batch_marker),
            runtime,
            filters: vec![],
            compatibility_filter,
        })
    }

    /// Validate one exact generated attribute scalar through the Rust projection contract.
    fn validate_attribute_value(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        value: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Attribute {
            return Err(py_type_error(
                "generated scalar validation requires an exact attribute class",
            ));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| py_runtime_error("projection attribute model is absent"))?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| py_runtime_error("projection attribute has no scalar domain"))?;
        let value = canonical_attribute_value_from_py(
            py,
            &value,
            projected_value_type(value_type),
            self.package.named_zone_marker.as_ref(),
        )?;
        ProjectedAttributeValue::try_from_attribute_value(&self.package.projection, id, &value)
            .map(|_| ())
            .map_err(py_sdk_diagnostic)
    }

    /// Validate one exact generated owned-field value through the Rust projection contract.
    fn validate_field_value(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        field_name: &str,
        value: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        let projected = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| py_runtime_error("projection model is absent"))?;
        let field = projected
            .query_tokens()
            .fields()
            .values()
            .find(|field| field.target_name().as_str() == field_name)
            .ok_or_else(|| {
                py_value_error("generated value references an unknown projected field")
            })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                .map_err(py_diagnostic)?;
        let (actual_id, form) = self.package.identify_value(py, &value).map_err(|_| {
            py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(field.id().clone()),
            ]))
        })?;
        if actual_id != attribute_id || form != ProjectedModelForm::Complete {
            return Err(py_type_error(
                "generated owned field requires its exact attribute wrapper",
            ));
        }
        let attribute = self
            .package
            .projection
            .projection()
            .models()
            .get(&attribute_id)
            .ok_or_else(|| py_runtime_error("projection field attribute is absent"))?;
        let value_type = attribute
            .declaration()
            .value_type()
            .ok_or_else(|| py_runtime_error("projection field attribute has no scalar domain"))?;
        let scalar = value.call_method0("runtime_attribute_value")?;
        let scalar = canonical_attribute_value_from_py(
            py,
            &scalar,
            projected_value_type(value_type),
            self.package.named_zone_marker.as_ref(),
        )?;
        let projected = ProjectedAttributeValue::try_from_attribute_value(
            &self.package.projection,
            attribute_id,
            &scalar,
        )
        .map_err(py_sdk_diagnostic)?;
        self.package
            .projection
            .validate_canonical_field_value(&id, field.id(), projected.value())
            .map_err(py_sdk_diagnostic)
    }

    /// Encode one exact generated attribute wrapper as canonical record bytes.
    fn encode_attribute<'py>(
        &self,
        py: Python<'py>,
        model: Py<PyType>,
        instance: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let expected = self.package.model_id_for_class(py, &model)?;
        if expected.kind() != TypeKind::Attribute {
            return Err(py_type_error(
                "canonical attribute encoding requires an exact generated attribute class",
            ));
        }
        let projected = project_attribute_value(py, self.package.as_ref(), &instance, &[])?;
        if projected.attribute_type() != &expected {
            return Err(py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(expected),
            ])));
        }
        let record = type_bridge_orm::record_from_attribute(&self.package.projection, &projected)
            .map_err(|error| py_value_error(error.to_string()))?;
        let bytes = record.encode().map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode canonical attribute bytes through exact installed package authority.
    fn decode_attribute(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        let expected = self.package.model_id_for_class(py, &model)?;
        if expected.kind() != TypeKind::Attribute {
            return Err(py_type_error(
                "canonical attribute decoding requires an exact generated attribute class",
            ));
        }
        let record =
            type_bridge_contract::projected_record::ProjectedRecord::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| py_value_error(error.to_string()))?;
        let ProjectedCodecValue::Attribute(value) = value else {
            return Err(py_value_error("canonical record is not an attribute value"));
        };
        if value.attribute_type() != &expected {
            return Err(py_value_error(
                "canonical attribute has the wrong exact generated type",
            ));
        }
        let scalar = attribute_value_to_py(
            py,
            &value.to_attribute_value(),
            self.package.named_zone_marker.as_ref(),
        )?;
        model.bind(py).call1((scalar,)).map(Bound::unbind)
    }

    /// Validate one complete generated create payload through the common Rust contract.
    fn validate_create(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        instance: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        let (instance_id, form) = self.package.identify_value(py, &instance).map_err(|_| {
            py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ]))
        })?;
        if instance_id != id || form != ProjectedModelForm::Complete {
            return Err(py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_create(py, self.package.as_ref(), &id, &instance).map(|_| ())
    }

    /// Encode one exact generated create payload as canonical record bytes.
    fn encode_create<'py>(
        &self,
        py: Python<'py>,
        model: Py<PyType>,
        instance: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let id = self.package.model_id_for_class(py, &model)?;
        let projected = project_create(py, self.package.as_ref(), &id, &instance)?;
        let record = type_bridge_orm::record_from_create(&self.package.projection, &projected)
            .map_err(|error| py_value_error(error.to_string()))?;
        let bytes = record.encode().map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode canonical create bytes through exact installed package authority.
    fn decode_create(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        let expected = self.package.model_id_for_class(py, &model)?;
        let record =
            type_bridge_contract::projected_record::ProjectedRecord::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| py_value_error(error.to_string()))?;
        let ProjectedCodecValue::Create(value) = value else {
            return Err(py_value_error("canonical record is not a create payload"));
        };
        if value.type_id() != &expected {
            return Err(py_value_error(
                "canonical create has the wrong exact generated type",
            ));
        }
        hydrate_projected_create(py, self.package.as_ref(), &value)
    }

    /// Encode one exact generated detached reference as canonical record bytes.
    fn encode_reference<'py>(
        &self,
        py: Python<'py>,
        model: Py<PyType>,
        instance: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let id = self.package.model_id_for_reference_class(py, &model)?;
        let allowed = BTreeSet::from([ProjectedModelUse::new(id, ProjectedModelForm::Reference)]);
        let projected = project_reference(py, self.package.as_ref(), &instance, &allowed, &[])?;
        let record = type_bridge_orm::record_from_reference(&self.package.projection, &projected)
            .map_err(|error| py_value_error(error.to_string()))?;
        let bytes = record.encode().map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode canonical reference bytes through exact installed package authority.
    fn decode_reference(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        let expected = self.package.model_id_for_reference_class(py, &model)?;
        let record =
            type_bridge_contract::projected_record::ProjectedRecord::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| py_value_error(error.to_string()))?;
        let ProjectedCodecValue::Reference(value) = value else {
            return Err(py_value_error("canonical record is not a reference"));
        };
        if value.type_id() != &expected {
            return Err(py_value_error(
                "canonical reference has the wrong exact generated type",
            ));
        }
        hydrate_projected_detached_reference(py, self.package.as_ref(), &value)
    }

    /// Encode one exact generated hydrated model as a detached canonical snapshot.
    fn encode_snapshot<'py>(
        &self,
        py: Python<'py>,
        model: Py<PyType>,
        instance: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let id = self.package.model_id_for_class(py, &model)?;
        let (actual, form) = self.package.identify_value(py, &instance)?;
        let iid = projected_iid(&instance)?;
        if actual != id || form != ProjectedModelForm::Complete || iid.is_none() {
            return Err(py_value_error(
                "snapshot requires the exact generated complete model with an IID",
            ));
        }
        let values = instance.call_method0("runtime_values")?;
        let values = values.cast_exact::<PyDict>()?;
        let projected =
            project_hydrated_thing(py, self.package.as_ref(), &id, values, iid.as_deref())?;
        let record = type_bridge_orm::record_from_snapshot(&self.package.projection, &projected)
            .map_err(|error| py_value_error(error.to_string()))?;
        let bytes = record.encode().map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode canonical snapshot bytes through exact installed package authority.
    fn decode_snapshot(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        let expected = self.package.model_id_for_class(py, &model)?;
        let record =
            type_bridge_contract::projected_record::ProjectedRecord::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| py_value_error(error.to_string()))?;
        let ProjectedCodecValue::Snapshot(value) = value else {
            return Err(py_value_error("canonical record is not a snapshot"));
        };
        if value.type_id() != &expected {
            return Err(py_value_error(
                "canonical snapshot has the wrong exact generated type",
            ));
        }
        hydrate_projected_thing_value(py, self.package.as_ref(), &value)
    }

    /// Encode one exact generated struct value as canonical record bytes.
    fn encode_struct<'py>(
        &self,
        py: Python<'py>,
        structure: Py<PyType>,
        instance: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let id = self.package.struct_id_for_class(py, &structure)?;
        if !instance.get_type().is(structure.bind(py)) {
            return Err(py_type_error(
                "struct value is not an instance of the exact registered class",
            ));
        }
        let projection = self
            .package
            .projection
            .projection()
            .structs()
            .values()
            .find(|projection| projection.id().label() == id.label())
            .ok_or_else(|| py_runtime_error("struct is absent from installed projection"))?;
        let members = projection
            .fields()
            .iter()
            .map(|field| {
                let value = instance.getattr(field.target_name().as_str())?;
                if value.is_none() {
                    return Ok(None);
                }
                canonical_attribute_value_from_py(
                    py,
                    &value,
                    projected_value_type(field.value_type()),
                    self.package.named_zone_marker.as_ref(),
                )
                .and_then(canonical_struct_scalar)
                .map(Some)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let projected =
            type_bridge_orm::ProjectedStructValue::try_new(&self.package.projection, id, members)
                .map_err(|error| py_value_error(error.to_string()))?;
        let record = type_bridge_orm::record_from_struct(&self.package.projection, &projected)
            .map_err(|error| py_value_error(error.to_string()))?;
        let bytes = record.encode().map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode canonical struct bytes through exact installed package authority.
    fn decode_struct(
        &self,
        py: Python<'_>,
        structure: Py<PyType>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        let expected = self.package.struct_id_for_class(py, &structure)?;
        let record =
            type_bridge_contract::projected_record::ProjectedRecord::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| py_value_error(error.to_string()))?;
        let ProjectedCodecValue::Struct(value) = value else {
            return Err(py_value_error("canonical record is not a struct"));
        };
        if value.type_id() != &expected {
            return Err(py_value_error(
                "canonical struct has the wrong exact generated type",
            ));
        }
        let projection = self
            .package
            .projection
            .projection()
            .structs()
            .values()
            .find(|projection| projection.id().label() == expected.label())
            .ok_or_else(|| py_runtime_error("struct is absent from installed projection"))?;
        let kwargs = PyDict::new(py);
        for (field, member) in projection.fields().iter().zip(value.members()) {
            let value = member
                .as_ref()
                .map(|value| {
                    canonical_struct_value_to_py(py, value, self.package.named_zone_marker.as_ref())
                })
                .transpose()?
                .unwrap_or_else(|| py.None());
            kwargs.set_item(field.target_name().as_str(), value)?;
        }
        structure
            .bind(py)
            .call((), Some(&kwargs))
            .map(Bound::unbind)
    }

    /// Encode one nominal record with cancellation, deadline, and resource limits.
    #[pyo3(signature = (record_kind, target, instance, *, cancellation=None, timeout_milliseconds=None, max_input_bytes=None, max_output_bytes=None, max_depth=None, max_members=None))]
    #[allow(clippy::too_many_arguments)]
    fn encode_record_controlled<'py>(
        &self,
        py: Python<'py>,
        record_kind: &str,
        target: Py<PyType>,
        instance: Bound<'py, PyAny>,
        cancellation: Option<&PyQueryCancellation>,
        timeout_milliseconds: Option<u64>,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
        max_depth: Option<usize>,
        max_members: Option<usize>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let control = PythonCanonicalControl::capture(
            cancellation,
            timeout_milliseconds,
            max_input_bytes,
            max_output_bytes,
            max_depth,
            None,
            max_members,
        )?;
        control.check()?;
        let bytes = match record_kind {
            "attribute" => self.encode_attribute(py, target, instance),
            "create" => self.encode_create(py, target, instance),
            "reference" => self.encode_reference(py, target, instance),
            "snapshot" => self.encode_snapshot(py, target, instance),
            "struct" => self.encode_struct(py, target, instance),
            _ => Err(py_value_error("unsupported canonical record kind")),
        }?;
        control.check()?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode_with_limits(
            bytes.as_bytes(),
            control.output_limits(),
        )
        .map_err(|error| python_canonical_limit_error(error, false))?;
        if record.decoded_weight() > control.max_members {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let bytes = record
            .encode_with_limits(control.output_limits())
            .map_err(|error| python_canonical_limit_error(error, false))?;
        control.check()?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode one nominal record with cancellation, deadline, and resource limits.
    #[pyo3(signature = (record_kind, target, data, *, cancellation=None, timeout_milliseconds=None, max_input_bytes=None, max_output_bytes=None, max_depth=None, max_members=None))]
    #[allow(clippy::too_many_arguments)]
    fn decode_record_controlled(
        &self,
        py: Python<'_>,
        record_kind: &str,
        target: Py<PyType>,
        data: &Bound<'_, PyBytes>,
        cancellation: Option<&PyQueryCancellation>,
        timeout_milliseconds: Option<u64>,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
        max_depth: Option<usize>,
        max_members: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let control = PythonCanonicalControl::capture(
            cancellation,
            timeout_milliseconds,
            max_input_bytes,
            max_output_bytes,
            max_depth,
            None,
            max_members,
        )?;
        control.check()?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode_with_limits(
            data.as_bytes(),
            control.input_limits(),
        )
        .map_err(|error| python_canonical_limit_error(error, true))?;
        if record.decoded_weight() > control.max_members {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let canonical = record
            .encode_with_limits(control.output_limits())
            .map_err(|error| python_canonical_limit_error(error, false))?;
        control.check()?;
        let canonical = PyBytes::new(py, &canonical);
        let value = match record_kind {
            "attribute" => self.decode_attribute(py, target, &canonical),
            "create" => self.decode_create(py, target, &canonical),
            "reference" => self.decode_reference(py, target, &canonical),
            "snapshot" => self.decode_snapshot(py, target, &canonical),
            "struct" => self.decode_struct(py, target, &canonical),
            _ => Err(py_value_error("unsupported canonical record kind")),
        }?;
        control.check()?;
        Ok(value)
    }

    /// Compose canonical exact-package records into one deterministic archive.
    fn encode_archive<'py>(
        &self,
        py: Python<'py>,
        records: Vec<Py<PyBytes>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let mut verified = Vec::with_capacity(records.len());
        for bytes in records {
            let record = type_bridge_contract::projected_record::ProjectedRecord::decode(
                bytes.bind(py).as_bytes(),
            )
            .map_err(py_diagnostic)?;
            let _ = type_bridge_orm::materialize_record(&self.package.projection, &record)
                .map_err(python_canonical_codec_error)?;
            verified.push(record);
        }
        let bytes = type_bridge_contract::projected_record::ProjectedArchive::try_new(verified)
            .and_then(|archive| archive.encode())
            .map_err(py_diagnostic)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Compose records with cancellation, deadline, and tighten-only limits.
    #[pyo3(signature = (records, *, cancellation=None, timeout_milliseconds=None, max_input_bytes=None, max_output_bytes=None, max_depth=None, max_records=None, max_members=None))]
    #[allow(clippy::too_many_arguments)]
    fn encode_archive_controlled<'py>(
        &self,
        py: Python<'py>,
        records: Vec<Py<PyBytes>>,
        cancellation: Option<&PyQueryCancellation>,
        timeout_milliseconds: Option<u64>,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
        max_depth: Option<usize>,
        max_records: Option<usize>,
        max_members: Option<usize>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let control = PythonCanonicalControl::capture(
            cancellation,
            timeout_milliseconds,
            max_input_bytes,
            max_output_bytes,
            max_depth,
            max_records,
            max_members,
        )?;
        control.check()?;
        if records.len() > control.max_records {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let mut verified = Vec::with_capacity(records.len());
        for bytes in records {
            control.check()?;
            let record =
                type_bridge_contract::projected_record::ProjectedRecord::decode_with_limits(
                    bytes.bind(py).as_bytes(),
                    control.input_limits(),
                )
                .map_err(|error| python_canonical_limit_error(error, true))?;
            let _ = type_bridge_orm::materialize_record(&self.package.projection, &record)
                .map_err(python_canonical_codec_error)?;
            verified.push(record);
        }
        let archive = type_bridge_contract::projected_record::ProjectedArchive::try_new(verified)
            .map_err(py_diagnostic)?;
        if archive.decoded_weight() > control.max_members {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        control.check()?;
        let bytes = archive
            .encode_with_limits(control.output_limits())
            .map_err(|error| python_canonical_limit_error(error, false))?;
        control.check()?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode one archive into canonical individual exact-package records.
    fn decode_archive<'py>(
        &self,
        py: Python<'py>,
        bytes: &Bound<'_, PyBytes>,
    ) -> PyResult<Bound<'py, PyList>> {
        let archive =
            type_bridge_contract::projected_record::ProjectedArchive::decode(bytes.as_bytes())
                .map_err(py_diagnostic)?;
        let records = archive
            .records()
            .iter()
            .map(|record| {
                let _ = type_bridge_orm::materialize_record(&self.package.projection, record)
                    .map_err(python_canonical_codec_error)?;
                record
                    .encode()
                    .map(|bytes| PyBytes::new(py, &bytes).unbind())
                    .map_err(py_diagnostic)
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyList::new(py, records)
    }

    /// Decode an archive with cancellation, deadline, and tighten-only limits.
    #[pyo3(signature = (bytes, *, cancellation=None, timeout_milliseconds=None, max_input_bytes=None, max_output_bytes=None, max_depth=None, max_records=None, max_members=None))]
    #[allow(clippy::too_many_arguments)]
    fn decode_archive_controlled<'py>(
        &self,
        py: Python<'py>,
        bytes: &Bound<'_, PyBytes>,
        cancellation: Option<&PyQueryCancellation>,
        timeout_milliseconds: Option<u64>,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
        max_depth: Option<usize>,
        max_records: Option<usize>,
        max_members: Option<usize>,
    ) -> PyResult<Bound<'py, PyList>> {
        let control = PythonCanonicalControl::capture(
            cancellation,
            timeout_milliseconds,
            max_input_bytes,
            max_output_bytes,
            max_depth,
            max_records,
            max_members,
        )?;
        control.check()?;
        let archive = type_bridge_contract::projected_record::ProjectedArchive::decode_with_limits(
            bytes.as_bytes(),
            control.input_limits(),
        )
        .map_err(|error| python_canonical_limit_error(error, true))?;
        if archive.records().len() > control.max_records
            || archive.decoded_weight() > control.max_members
        {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let mut output_bytes = 0_usize;
        let records = archive
            .records()
            .iter()
            .map(|record| {
                control.check()?;
                let _ = type_bridge_orm::materialize_record(&self.package.projection, record)
                    .map_err(python_canonical_codec_error)?;
                let bytes = record
                    .encode_with_limits(control.output_limits())
                    .map_err(|error| python_canonical_limit_error(error, false))?;
                output_bytes = output_bytes.saturating_add(bytes.len());
                if output_bytes > control.max_output_bytes {
                    return Err(py_sdk_diagnostic(
                        SdkExecutionDiagnostic::projected_codec_output_limit(),
                    ));
                }
                Ok(PyBytes::new(py, &bytes).unbind())
            })
            .collect::<PyResult<Vec<_>>>()?;
        control.check()?;
        PyList::new(py, records)
    }

    /// Compile a retained raw-query entity match from one exact generated class.
    fn query_builder_match_entity(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        variable: &str,
        filters: &Bound<'_, PyDict>,
    ) -> PyResult<String> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Entity {
            return Err(py_type_error(
                "generated entity matches require an exact installed entity class",
            ));
        }
        let descriptor = self
            .package
            .projection
            .entity_descriptor(&id)
            .map_err(py_orm_error)?;
        let mut constraints = Vec::with_capacity(filters.len());
        for (field_name, value) in filters.iter() {
            let field_name: String = field_name
                .cast_exact::<PyString>()
                .map_err(|_| py_type_error("generated entity filter names must be exact strings"))?
                .extract()?;
            let field = descriptor
                .owned_attributes
                .iter()
                .find(|field| field.field_name == field_name)
                .ok_or_else(|| {
                    py_value_error(format!(
                        "generated entity {:?} has no projected field {field_name:?}",
                        id.label().as_str()
                    ))
                })?;
            let attribute_id = self
                .package
                .type_by_label(&field.attr_name, TypeKind::Attribute)?;
            let attribute_class = self
                .package
                .class(attribute_id, ProjectedModelForm::Complete)?;
            let scalar = if value.get_type().as_ptr() == attribute_class.bind(py).as_ptr() {
                value.call_method0("runtime_attribute_value")?
            } else {
                value
            };
            let value = attribute_value_from_py(py, &scalar, field.value_type)?;
            constraints.push(Constraint::Has {
                attr_name: field.attr_name.clone(),
                value: value.to_ast_value(),
            });
        }
        compatibility_clause_body(
            Clause::Match(vec![Pattern::Entity {
                variable: variable.to_owned(),
                type_name: id.label().as_str().to_owned(),
                constraints,
                is_strict: false,
            }]),
            "match",
        )
    }

    /// Compile a retained raw-query entity insert from one exact generated value.
    fn query_builder_insert_entity(
        &self,
        py: Python<'_>,
        instance: Bound<'_, PyAny>,
        variable: &str,
    ) -> PyResult<String> {
        let (id, form) = self.package.identify_value(py, &instance)?;
        if id.kind() != TypeKind::Entity || form != ProjectedModelForm::Complete {
            return Err(py_type_error(
                "generated entity inserts require an exact installed complete entity",
            ));
        }
        let descriptor = self
            .package
            .projection
            .entity_descriptor(&id)
            .map_err(py_orm_error)?;
        let attributes = lower_attributes(
            py,
            self.package.as_ref(),
            &descriptor.owned_attributes,
            &instance,
        )?;
        let mut statements = vec![Statement::Isa {
            variable: variable.to_owned(),
            type_name: id.label().as_str().to_owned(),
        }];
        statements.extend(
            attributes
                .into_iter()
                .map(|(attribute, value)| Statement::Has {
                    subject_var: variable.to_owned(),
                    attr_name: attribute,
                    value: value.to_ast_value(),
                }),
        );
        compatibility_clause_body(Clause::Insert(statements), "insert")
    }

    /// Compile a retained raw-query relation match from generated role tokens.
    #[pyo3(signature = (model, variable, role_players=None))]
    fn query_builder_match_relation(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        variable: &str,
        role_players: Option<Bound<'_, PyDict>>,
    ) -> PyResult<String> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Relation {
            return Err(py_type_error(
                "generated relation matches require an exact installed relation class",
            ));
        }
        let projected = &self.package.projection.projection().models()[&id];
        let mut players = Vec::new();
        if let Some(role_players) = role_players {
            players.reserve(role_players.len());
            for (role_name, player_variable) in role_players.iter() {
                let role_name: String = role_name
                    .cast_exact::<PyString>()
                    .map_err(|_| {
                        py_type_error("generated relation role names must be exact strings")
                    })?
                    .extract()?;
                let token = projected
                    .query_tokens()
                    .roles()
                    .values()
                    .find(|token| token.target_name().as_str() == role_name)
                    .ok_or_else(|| {
                        py_value_error(format!(
                            "generated relation {:?} has no projected role {role_name:?}",
                            id.label().as_str()
                        ))
                    })?;
                let player_variable: String = player_variable
                    .cast_exact::<PyString>()
                    .map_err(|_| {
                        py_type_error("generated relation player variables must be exact strings")
                    })?
                    .extract()?;
                players.push(RolePlayer {
                    role: token.role().label().as_str().to_owned(),
                    player_var: player_variable,
                });
            }
        }
        compatibility_clause_body(
            Clause::Match(vec![Pattern::Relation {
                variable: variable.to_owned(),
                type_name: id.label().as_str().to_owned(),
                role_players: players,
                constraints: Vec::new(),
            }]),
            "match",
        )
    }

    /// Build an opaque match session from this exact installed projection only.
    fn match_session(&self) -> PyResult<PyMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(py_orm_error)?;
        Ok(PyMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            type_bridge_orm::QueryExecutionResourceLimits::default(),
            type_bridge_orm::AnswerCancellation::default(),
        ))
    }

    /// Build a match session with one common direct/remote resource policy
    /// and caller-owned cooperative cancellation owner.
    fn match_session_with_resources(
        &self,
        resources: PyRef<'_, PyQueryExecutionResourceLimits>,
        cancellation: PyRef<'_, PyQueryCancellation>,
    ) -> PyResult<PyMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(py_orm_error)?;
        Ok(PyMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            resources.inner(),
            cancellation.inner(),
        ))
    }

    /// Hydrate one proof-backed query result through this exact package projection.
    fn hydrate_thing(
        &self,
        py: Python<'_>,
        thing: PyRef<'_, PyValidatedMatchThingHandle>,
    ) -> PyResult<Py<PyAny>> {
        hydrate_validated_thing(py, self.package.as_ref(), &thing)
    }
}

/// Exact CRUD manager for one generated projected entity or relation class.
#[pyclass]
pub struct PyProjectedModelManager {
    package: Arc<InstalledPackage>,
    type_id: TypeId,
    database: Option<Arc<Database>>,
    transaction: Option<TransactionContext>,
    successor_batch_marker: Option<Arc<AtomicBool>>,
    runtime: Arc<ProviderRuntimeOwner>,
    filters: Vec<DynamicExpr>,
    compatibility_filter: Option<ProjectedManagerFilter>,
}

/// Persistent canonical filter for one exact generated model.
#[pyclass]
pub struct PyProjectedManagerFilter {
    package: Arc<InstalledPackage>,
    database: Option<Arc<Database>>,
    transaction: Option<TransactionContext>,
    runtime: Arc<ProviderRuntimeOwner>,
    filter: ProjectedManagerFilter,
}

#[pymethods]
impl PyProjectedManagerFilter {
    /// Append one package-issued field predicate without mutating this filter.
    #[pyo3(signature = (owner, field_name, metadata_json, comparison, value))]
    fn where_field(
        &self,
        py: Python<'_>,
        owner: Py<PyType>,
        field_name: &str,
        metadata_json: &str,
        comparison: &str,
        value: Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let owner_id = self.package.model_id_for_class(py, &owner).map_err(|_| {
            py_sdk_diagnostic(SdkExecutionDiagnostic::generated_token_package_mismatch())
        })?;
        let field = self
            .package
            .projection
            .projection()
            .models()
            .get(&owner_id)
            .and_then(|model| {
                model
                    .query_tokens()
                    .fields()
                    .values()
                    .find(|field| field.target_name().as_str() == field_name)
            })
            .ok_or_else(|| {
                py_sdk_diagnostic(SdkExecutionDiagnostic::generated_token_package_mismatch())
            })?;
        if !to_canonical_json(field)
            .is_ok_and(|canonical| canonical.as_slice() == metadata_json.as_bytes())
        {
            return Err(py_sdk_diagnostic(
                SdkExecutionDiagnostic::generated_token_package_mismatch(),
            ));
        }
        let projected = project_attribute_value(py, self.package.as_ref(), &value, &[])?;
        let comparison = projected_manager_comparison(comparison)?;
        let filter = self
            .filter
            .try_and(
                &self.package.projection,
                &ProjectedTokenIdentity::Field {
                    owner: owner_id,
                    field: field.id().clone(),
                },
                comparison,
                &projected,
            )
            .map_err(py_sdk_diagnostic)?;
        Ok(Self {
            package: Arc::clone(&self.package),
            database: self.database.clone(),
            transaction: self.transaction.clone(),
            runtime: Arc::clone(&self.runtime),
            filter,
        })
    }

    /// Reject a token that was not issued by the calling generated package.
    fn reject_unissued_field(&self) -> PyResult<Self> {
        Err(py_sdk_diagnostic(
            SdkExecutionDiagnostic::generated_token_package_mismatch(),
        ))
    }

    /// Hydrate every distinct exact-model match.
    fn all(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let executor = ProjectedManagerFilterExecutor::new(&self.package.projection);
        let projected = match (&self.database, &self.transaction) {
            (Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.all(
                    database.as_ref(),
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            (None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.all_in_read_transaction(
                    transaction,
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            _ => return Err(py_runtime_error("projected filter has no execution target")),
        }
        .map_err(py_sdk_diagnostic)?;
        let values = PyList::empty(py);
        for value in projected {
            values.append(hydrate_projected_thing(py, self.package.as_ref(), value)?)?;
        }
        Ok(values.into_any().unbind())
    }

    /// Return the optional identity-proven match.
    fn first(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let executor = ProjectedManagerFilterExecutor::new(&self.package.projection);
        let projected = match (&self.database, &self.transaction) {
            (Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.first(
                    database.as_ref(),
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            (None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.first_in_read_transaction(
                    transaction,
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            _ => return Err(py_runtime_error("projected filter has no execution target")),
        }
        .map_err(py_sdk_diagnostic)?;
        projected
            .map(|value| hydrate_projected_thing(py, self.package.as_ref(), value))
            .transpose()
            .map(|value| value.unwrap_or_else(|| py.None()))
    }

    /// Count distinct exact-model matches.
    fn count(&self, py: Python<'_>) -> PyResult<u64> {
        let executor = ProjectedManagerFilterExecutor::new(&self.package.projection);
        match (&self.database, &self.transaction) {
            (Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.count(
                    database.as_ref(),
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            (None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.count_in_read_transaction(
                    transaction,
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            _ => return Err(py_runtime_error("projected filter has no execution target")),
        }
        .map_err(py_sdk_diagnostic)
    }

    /// Return whether at least one exact-model match exists.
    fn exists(&self, py: Python<'_>) -> PyResult<bool> {
        let executor = ProjectedManagerFilterExecutor::new(&self.package.projection);
        match (&self.database, &self.transaction) {
            (Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.exists(
                    database.as_ref(),
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            (None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.exists_in_read_transaction(
                    transaction,
                    &self.filter,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            ),
            _ => return Err(py_runtime_error("projected filter has no execution target")),
        }
        .map_err(py_sdk_diagnostic)
    }
}

#[pymethods]
impl PyProjectedModelManager {
    /// Start a distinct persistent canonical filter for this exact model.
    fn canonical_filter(&self) -> PyResult<PyProjectedManagerFilter> {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.tx_type() != type_bridge_orm::TxType::Read)
        {
            let diagnostic = SdkExecutionDiagnostic::invalid_input(
                SdkDiagnosticCode::new("borrowed_target_not_read_only")
                    .expect("the static borrowed-target code is canonical"),
                SdkDiagnosticMessage::new(
                    "Canonical manager filters require a borrowed read transaction",
                )
                .expect("the static borrowed-target message is canonical"),
            )
            .try_at(SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::Operation,
            ))
            .expect("the fixed borrowed-target path fits the SDK diagnostic contract");
            return Err(py_sdk_diagnostic(diagnostic));
        }
        let filter =
            ProjectedManagerFilter::try_new(&self.package.projection, self.type_id.clone())
                .map_err(py_sdk_diagnostic)?;
        Ok(PyProjectedManagerFilter {
            package: Arc::clone(&self.package),
            database: self.database.clone(),
            transaction: self.transaction.clone(),
            runtime: Arc::clone(&self.runtime),
            filter,
        })
    }

    /// Insert one exact generated model and attach the returned TypeDB IID.
    fn insert(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.ensure_instance(py, &instance)?;
        self.validate_ordered_create(py, &instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.insert_projected(py, &input)?;
            return self.publish_projected_write(instance, prepared, snapshot, projected);
        }
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.insert(&attributes))
                    .map_err(py_orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.insert(&attributes, &players),
                )
                .map_err(py_orm_error)?
            }
        };
        instance.call_method1("attach_runtime_iid", (iid,))?;
        Ok(instance.unbind())
    }

    /// Insert exact generated models atomically and attach IIDs in input order.
    fn insert_many(&self, py: Python<'_>, instances: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if self.uses_successor_runtime() {
            return self.write_many_projected(
                py,
                bounded_projected_facades(&instances)?,
                ProjectedBatchOperation::Insert,
            );
        }
        self.write_many(py, instances.extract()?, false)
    }

    /// Insert or update one exact generated model and attach its TypeDB IID.
    fn put(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.ensure_instance(py, &instance)?;
        self.validate_ordered_create(py, &instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.put_projected(py, &input)?;
            return self.publish_projected_write(instance, prepared, snapshot, projected);
        }
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.put_exact(&attributes))
                    .map_err(py_orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.put_exact(&attributes, &players),
                )
                .map_err(py_orm_error)?
            }
        };
        instance.call_method1("attach_runtime_iid", (iid,))?;
        Ok(instance.unbind())
    }

    /// Put exact generated models atomically and attach IIDs in input order.
    fn put_many(&self, py: Python<'_>, instances: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if self.uses_successor_runtime() {
            return self.write_many_projected(
                py,
                bounded_projected_facades(&instances)?,
                ProjectedBatchOperation::Put,
            );
        }
        self.write_many(py, instances.extract()?, true)
    }

    /// Replace one exact generated model already identified by its TypeDB IID.
    fn update(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.ensure_instance(py, &instance)?;
        self.validate_ordered_create(py, &instance)?;
        let iid = required_projected_iid(&instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.update_projected(py, &iid, &input)?;
            let hydrated = hydrate_projected_thing_value(py, self.package.as_ref(), &projected)?;
            replace_projected_instance_atomic(py, &instance, hydrated, &snapshot)?;
            self.package
                .facade_origins
                .install(prepared, FacadeProjectionProof::Thing(Arc::new(projected)));
            return Ok(instance.unbind());
        }
        let hydrated = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_and_get_exact(&iid, &attributes),
                )
                .map_err(py_orm_error)?;
                hydrate_entity(py, self.package.as_ref(), &self.type_id, &row)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_and_get_exact(&iid, &attributes, &players),
                )
                .map_err(py_orm_error)?;
                hydrate_relation(py, self.package.as_ref(), &self.type_id, &row)?
            }
        };
        replace_projected_instance(py, instance, hydrated)
    }

    /// Replace exact generated models atomically and rehydrate them in input order.
    fn update_many(&self, py: Python<'_>, instances: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if self.uses_successor_runtime() {
            return self.write_many_projected(
                py,
                bounded_projected_facades(&instances)?,
                ProjectedBatchOperation::Update,
            );
        }
        let instances: Vec<Py<PyAny>> = instances.extract()?;
        if instances.is_empty() {
            return Ok(PyList::empty(py).into_any().unbind());
        }
        for instance in &instances {
            self.ensure_instance(py, instance.bind(py))?;
        }
        let hydrated = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            required_projected_iid(instance.bind(py))?,
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_many_and_get_exact(&items),
                )
                .map_err(py_orm_error)?
                .iter()
                .map(|row| hydrate_entity(py, self.package.as_ref(), &self.type_id, row))
                .collect::<PyResult<Vec<_>>>()?
            }
            TypeDescriptor::Relation(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            required_projected_iid(instance.bind(py))?,
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                            lower_roles(
                                py,
                                self.package.as_ref(),
                                &self.type_id,
                                &descriptor,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_many_and_get_exact(&items),
                )
                .map_err(py_orm_error)?
                .iter()
                .map(|row| hydrate_relation(py, self.package.as_ref(), &self.type_id, row))
                .collect::<PyResult<Vec<_>>>()?
            }
        };
        if hydrated.len() != instances.len() {
            return Err(py_runtime_error(
                "projected batch update returned an unexpected model count",
            ));
        }
        for (instance, stored) in instances.iter().zip(hydrated) {
            replace_projected_instance(py, instance.bind(py).clone(), stored)?;
        }
        Ok(PyList::new(py, &instances)?.into_any().unbind())
    }

    /// Resolve an IID-less exact generated model through its projected identity.
    ///
    /// Entities use every declared key. Relations use every populated owned
    /// attribute and role player, matching the detached-instance behavior of
    /// the pre-cutover manager without accepting handwritten descriptors.
    fn resolve_iid(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<Option<String>> {
        self.ensure_instance(py, &instance)?;
        if let Some(iid) = projected_iid(&instance)? {
            return Ok(Some(iid));
        }
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let keys = descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .collect::<Vec<_>>();
                if keys.is_empty() {
                    return Err(py_value_error(format!(
                        "generated entity {:?} requires an attached IID or projected key",
                        descriptor.type_name
                    )));
                }
                let mut expressions = Vec::with_capacity(keys.len());
                for key in keys {
                    let value = attributes
                        .iter()
                        .find_map(|(name, value)| (name == &key.attr_name).then(|| value.clone()))
                        .ok_or_else(|| {
                            py_value_error(format!(
                                "generated entity {:?} requires projected key {:?}",
                                descriptor.type_name, key.field_name
                            ))
                        })?;
                    expressions.push(DynamicExpr::Compare {
                        attr_name: key.attr_name.clone(),
                        operator: DynamicComparisonOp::Eq,
                        value,
                    });
                }
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&expressions, &[], Some(1), None),
                )
                .map_err(py_orm_error)?;
                resolved_entity_iid(rows.first())
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let key_names = descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .map(|attribute| attribute.attr_name.clone())
                    .collect::<BTreeSet<_>>();
                let mut expressions = attributes
                    .into_iter()
                    .filter(|(attr_name, _)| key_names.is_empty() || key_names.contains(attr_name))
                    .map(|(attr_name, value)| DynamicExpr::Compare {
                        attr_name,
                        operator: DynamicComparisonOp::Eq,
                        value,
                    })
                    .collect::<Vec<_>>();
                if key_names.is_empty() {
                    let players = lower_roles(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &descriptor,
                        &instance,
                    )?;
                    expressions.extend(players.into_iter().map(|player| {
                        let expr = match (player.iid, player.key) {
                            (Some(iid), None) => DynamicExpr::Iid { iid },
                            (None, Some((attr_name, value))) => DynamicExpr::Compare {
                                attr_name,
                                operator: DynamicComparisonOp::Eq,
                                value,
                            },
                            _ => unreachable!("lower_roles enforces exactly one player identity"),
                        };
                        DynamicExpr::RolePlayer {
                            role_name: player.role_name,
                            expr: Box::new(expr),
                        }
                    }));
                }
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&expressions, &[], Some(1), None),
                )
                .map_err(py_orm_error)?;
                resolved_relation_iid(rows.first())
            }
        }
    }

    /// Delete one exact generated model by its instance or canonical TypeDB IID.
    fn delete(&self, py: Python<'_>, instance_or_iid: Bound<'_, PyAny>) -> PyResult<()> {
        let iid = if let Ok(iid) = instance_or_iid.cast_exact::<PyString>() {
            iid.to_str()?.to_owned()
        } else {
            self.ensure_instance(py, &instance_or_iid)?;
            required_projected_iid(&instance_or_iid)?
        };
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.delete_by_iid_exact(&iid))
                    .map_err(py_orm_error)?;
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.delete_by_iid_exact(&iid))
                    .map_err(py_orm_error)?;
            }
        }
        Ok(())
    }

    /// Delete exact generated models atomically by canonical IID.
    fn delete_many(&self, py: Python<'_>, iids: Bound<'_, PyAny>) -> PyResult<()> {
        if self.uses_successor_runtime() {
            return self.delete_many_projected(py, bounded_projected_iids(&iids)?);
        }
        let iids: Vec<String> = iids.extract()?;
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.delete_many_by_iid_exact(&iids),
                )
                .map_err(py_orm_error)?;
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.delete_many_by_iid_exact(&iids),
                )
                .map_err(py_orm_error)?;
            }
        }
        Ok(())
    }

    /// Return a new exact generated-model manager narrowed by keyword filters.
    #[pyo3(signature = (**filters))]
    fn filter(&self, py: Python<'_>, filters: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let descriptor = self.descriptor()?;
        let attributes = match &descriptor {
            TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
            TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
        };
        let mut combined = self.filters.clone();
        combined.extend(lower_filter_kwargs(
            py,
            self.package.as_ref(),
            attributes,
            filters,
        )?);
        let compatibility_filter = match &self.compatibility_filter {
            Some(filter) => lower_compatibility_filter_kwargs(
                py,
                self.package.as_ref(),
                &self.type_id,
                attributes,
                filters,
                filter,
            )?,
            None => None,
        };
        Ok(Self {
            package: Arc::clone(&self.package),
            type_id: self.type_id.clone(),
            database: self.database.clone(),
            transaction: self.transaction.clone(),
            successor_batch_marker: self.successor_batch_marker.clone(),
            runtime: Arc::clone(&self.runtime),
            filters: combined,
            compatibility_filter,
        })
    }

    /// Fetch all exact instances of this projected type using `isa!`.
    fn all(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if let Some(filter) = &self.compatibility_filter {
            return self.compatibility_filter_handle(filter.clone()).all(py);
        }
        self.legacy_all(py)
    }

    /// Preserve released dynamic-query selection for generated mutations.
    fn legacy_all(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&self.filters, &[], None, None),
                )
                .map_err(py_orm_error)?;
                let values = PyList::empty(py);
                for row in rows {
                    values.append(hydrate_entity(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &row,
                    )?)?;
                }
                Ok(values.into_any().unbind())
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&self.filters, &[], None, None),
                )
                .map_err(py_orm_error)?;
                let values = PyList::empty(py);
                for row in rows {
                    values.append(hydrate_relation(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &row,
                    )?)?;
                }
                Ok(values.into_any().unbind())
            }
        }
    }

    /// Return the first exact filtered model, or `None` when no model matches.
    fn first(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.first_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)?;
                row.as_ref()
                    .map(|row| hydrate_entity(py, self.package.as_ref(), &self.type_id, row))
                    .transpose()
                    .map(|value| value.unwrap_or_else(|| py.None()))
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.first_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)?;
                row.as_ref()
                    .map(|row| hydrate_relation(py, self.package.as_ref(), &self.type_id, row))
                    .transpose()
                    .map(|value| value.unwrap_or_else(|| py.None()))
            }
        }
    }

    /// Count exact filtered models.
    fn count(&self, py: Python<'_>) -> PyResult<u64> {
        if let Some(filter) = &self.compatibility_filter {
            return self.compatibility_filter_handle(filter.clone()).count(py);
        }
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.count_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.count_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
        }
    }

    /// Return whether at least one exact filtered model exists.
    fn exists(&self, py: Python<'_>) -> PyResult<bool> {
        if let Some(filter) = &self.compatibility_filter {
            return self.compatibility_filter_handle(filter.clone()).exists(py);
        }
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.exists_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.exists_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
        }
    }

    /// Fetch one exact instance by TypeDB IID using `isa!`.
    fn get_by_iid(&self, py: Python<'_>, iid: &str) -> PyResult<Py<PyAny>> {
        // Preserve the released Python manager contract: malformed IIDs are
        // indistinguishable from absent IIDs for this convenience lookup.
        // Query predicates remain strict and reject malformed IIDs before I/O.
        if !is_canonical_thing_iid(iid) {
            return Ok(py.None());
        }
        if self.uses_successor_runtime() {
            return match self.get_projected(py, iid)? {
                Some(projected) => {
                    hydrate_projected_thing(py, self.package.as_ref(), Arc::new(projected))
                }
                None => Ok(py.None()),
            };
        }
        let iid = iid.to_owned();
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row =
                    provider_block_on(py, self.runtime.as_ref(), manager.get_by_iid_exact(&iid))
                        .map_err(py_orm_error)?;
                match row {
                    Some(row) => hydrate_entity(py, self.package.as_ref(), &self.type_id, &row),
                    None => Ok(py.None()),
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows =
                    provider_block_on(py, self.runtime.as_ref(), manager.get_by_iid_exact(&iid))
                        .map_err(py_orm_error)?;
                match rows.as_slice() {
                    [] => Ok(py.None()),
                    [row] => hydrate_relation(py, self.package.as_ref(), &self.type_id, row),
                    _ => Err(py_runtime_error(
                        "exact IID relation query returned multiple rows",
                    )),
                }
            }
        }
    }
}

impl PyProjectedModelManager {
    fn compatibility_filter_handle(
        &self,
        filter: ProjectedManagerFilter,
    ) -> PyProjectedManagerFilter {
        PyProjectedManagerFilter {
            package: Arc::clone(&self.package),
            database: self.database.clone(),
            transaction: self.transaction.clone(),
            runtime: Arc::clone(&self.runtime),
            filter,
        }
    }

    fn uses_successor_runtime(&self) -> bool {
        self.package.projection.projection().generator_handlers()
            == [type_bridge_contract::projection::ProjectionHandler::python_v2()]
    }

    fn insert_projected(
        &self,
        py: Python<'_>,
        input: &ProjectedCreate,
    ) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_entity(database.as_ref(), input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_entity_in_transaction(transaction, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_relation(database.as_ref(), input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_relation_in_transaction(transaction, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn put_projected(&self, py: Python<'_>, input: &ProjectedCreate) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_entity(database.as_ref(), input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_entity_in_transaction(transaction, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_relation(database.as_ref(), input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_relation_in_transaction(transaction, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn update_projected(
        &self,
        py: Python<'_>,
        iid: &str,
        input: &ProjectedCreate,
    ) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_entity(database.as_ref(), iid, input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_entity_in_transaction(transaction, iid, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_relation(database.as_ref(), iid, input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_relation_in_transaction(transaction, iid, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn get_projected(&self, py: Python<'_>, iid: &str) -> PyResult<Option<ProjectedThing>> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_entity_by_iid(database.as_ref(), &self.type_id, iid),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_entity_by_iid_in_transaction(transaction, &self.type_id, iid),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_relation_by_iid(database.as_ref(), &self.type_id, iid),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_relation_by_iid_in_transaction(transaction, &self.type_id, iid),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn publish_projected_write(
        &self,
        instance: Bound<'_, PyAny>,
        prepared: PreparedFacadeOrigin,
        snapshot: ProjectedFacadeSnapshot,
        projected: ProjectedThing,
    ) -> PyResult<Py<PyAny>> {
        let py = instance.py();
        let hydrated = hydrate_projected_thing_value(py, self.package.as_ref(), &projected)?;
        replace_projected_instance_atomic(py, &instance, hydrated, &snapshot)?;
        self.package
            .facade_origins
            .install(prepared, FacadeProjectionProof::Thing(Arc::new(projected)));
        Ok(instance.unbind())
    }

    fn validate_ordered_create(&self, py: Python<'_>, instance: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.uses_successor_runtime() {
            project_create(py, self.package.as_ref(), &self.type_id, instance)?;
        }
        Ok(())
    }

    fn write_many(
        &self,
        py: Python<'_>,
        instances: Vec<Py<PyAny>>,
        put: bool,
    ) -> PyResult<Py<PyAny>> {
        if instances.is_empty() {
            return Ok(PyList::empty(py).into_any().unbind());
        }
        for instance in &instances {
            self.ensure_instance(py, instance.bind(py))?;
        }
        let iids = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        lower_attributes(
                            py,
                            self.package.as_ref(),
                            &descriptor.owned_attributes,
                            instance.bind(py),
                        )
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                if put {
                    provider_block_on(py, self.runtime.as_ref(), manager.put_many_exact(&items))
                        .map_err(py_orm_error)?
                } else {
                    provider_block_on(py, self.runtime.as_ref(), manager.insert_many(&items))
                        .map_err(py_orm_error)?
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                            lower_roles(
                                py,
                                self.package.as_ref(),
                                &self.type_id,
                                &descriptor,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                if put {
                    provider_block_on(py, self.runtime.as_ref(), manager.put_many_exact(&items))
                        .map_err(py_orm_error)?
                } else {
                    provider_block_on(py, self.runtime.as_ref(), manager.insert_many(&items))
                        .map_err(py_orm_error)?
                }
            }
        };
        if iids.len() != instances.len() {
            return Err(py_runtime_error(
                "projected batch write returned an unexpected IID count",
            ));
        }
        for (instance, iid) in instances.iter().zip(iids) {
            instance
                .bind(py)
                .call_method1("attach_runtime_iid", (iid,))?;
        }
        Ok(PyList::new(py, &instances)?.into_any().unbind())
    }

    fn write_many_projected(
        &self,
        py: Python<'_>,
        instances: Vec<Py<PyAny>>,
        operation: ProjectedBatchOperation,
    ) -> PyResult<Py<PyAny>> {
        debug_assert!(matches!(
            operation,
            ProjectedBatchOperation::Insert
                | ProjectedBatchOperation::Put
                | ProjectedBatchOperation::Update
        ));
        ProjectedBatch::validate_binding_row_count(instances.len()).map_err(py_sdk_diagnostic)?;
        let package = Arc::clone(&self.package);
        let type_id = self.type_id.clone();
        let slots = self.package.batch_slots(&self.type_id)?;
        let database = self.database.clone();
        let transaction = self.transaction.clone();
        let successor_batch_marker = self.successor_batch_marker.clone();
        let runtime = Arc::clone(&self.runtime);
        provider_block_on_with_gil(py, runtime.as_ref(), move |py| {
            Box::pin(async move {
                let mut retired =
                    reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;
                for instance in &instances {
                    if let Some(snapshot) = package.facade_origins.take_retired(instance.bind(py)) {
                        retired.push(snapshot);
                    }
                }
                // Retired values can own hostile finalizers. Drain them before
                // snapshots, lowering, GC fencing, or mutation-lease entry.
                drop(retired);
                let control = ProjectedBatchInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                );
                let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
                    package.projection.as_ref(),
                    type_id.clone(),
                    operation,
                    &control,
                )
                .map_err(py_sdk_diagnostic)?;
                let mut staged_facades =
                    reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;
                let mut rows = reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;
                let mut identities =
                    reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;

                for (ordinal, instance) in instances.into_iter().enumerate() {
                    budget.checkpoint().map_err(py_sdk_diagnostic)?;
                    let instance = instance.bind(py);
                    ensure_batch_instance_at(py, package.as_ref(), &type_id, instance, ordinal)?;
                    let snapshot = slots.snapshot(py, instance).map_err(|_| {
                        py_sdk_diagnostic(batch_input_shape_diagnostic(
                            "generated_model_layout_mismatch",
                            "The exact generated model no longer has its installed runtime slots",
                            &projected_batch_row_path(ordinal),
                        ))
                    })?;
                    let path = projected_batch_row_path(ordinal);
                    let replacement = project_create_from_snapshot_at(
                        py,
                        package.as_ref(),
                        &type_id,
                        &snapshot,
                        &path,
                        &budget,
                    )?;
                    let row = if operation == ProjectedBatchOperation::Update {
                        ProjectedBatchRow::Update {
                            iid: projected_iid_from_snapshot(py, &snapshot, ordinal)?,
                            replacement,
                        }
                    } else {
                        ProjectedBatchRow::Create(replacement)
                    };
                    budget
                        .try_add_row(package.projection.as_ref(), &row)
                        .map_err(py_sdk_diagnostic)?;
                    rows.push(row);
                    identities.push((instance.as_ptr() as usize, ordinal));
                    staged_facades.push(StagedBatchFacade {
                        snapshot,
                        instance: instance.clone().unbind(),
                    });
                }
                drop(budget);
                let batch = ProjectedBatch::try_new_for_invocation(
                    package.projection.as_ref(),
                    type_id.clone(),
                    operation,
                    rows,
                    &control,
                )
                .map_err(py_sdk_diagnostic)?;
                // Common duplicate-key/target precedence is authoritative.
                // Only aliases that survive it are binding-unrepresentable.
                reject_duplicate_batch_facades(&mut identities)?;
                if batch.is_empty() {
                    let empty = fallible_python_empty_list(py)?;
                    let executor = ProjectedBatchExecutor::new(package.projection.as_ref());
                    let result = match (&database, &transaction) {
                        (Some(database), None) => {
                            executor
                                .execute_mapped(database.as_ref(), &batch, control, empty, |_| {
                                    Err(SdkExecutionDiagnostic::internal_failure())
                                })
                                .await
                        }
                        (None, Some(transaction)) => {
                            executor
                                .execute_in_transaction_mapped(
                                    transaction,
                                    &batch,
                                    control,
                                    empty,
                                    |_| Err(SdkExecutionDiagnostic::internal_failure()),
                                )
                                .await
                        }
                        _ => Err(SdkExecutionDiagnostic::internal_failure()),
                    };
                    return result.map_err(py_sdk_diagnostic);
                }
                let _gc_guard = SuccessorBatchGcGuard::disable(py);
                let hydration_pool =
                    BatchHydrationPool::try_new(&staged_facades).map_err(py_sdk_diagnostic)?;
                package.validate_batch_hydration_layouts(py).map_err(|_| {
                    py_sdk_diagnostic(batch_input_shape_diagnostic(
                        "generated_model_layout_mismatch",
                        "An installed successor hydration class changed before execution",
                        &[SdkDiagnosticPathSegment::Argument(
                            SdkDiagnosticName::new("rows")
                                .expect("the fixed batch argument is canonical"),
                        )],
                    ))
                })?;
                validate_staged_batch_facades_after_lowering(py, slots.as_ref(), &staged_facades)?;
                let mut facades =
                    reserved_binding_rows(staged_facades.len()).map_err(py_sdk_diagnostic)?;
                for staged in staged_facades {
                    let instance = staged.instance.bind(py);
                    facades.push(PreparedBatchFacade {
                        origin: package.facade_origins.prepare(instance)?,
                        snapshot: staged.snapshot,
                        instance: instance.clone().unbind(),
                    });
                }

                let empty = BatchMappedOutput::Empty(fallible_python_empty_list(py)?);
                let binding_error = Mutex::new(None);
                let mapper_package = Arc::clone(&package);
                let mapper_slots = Arc::clone(&slots);
                let mapper_error = &binding_error;
                let mapper_marker = successor_batch_marker.clone();
                let mapper_pool = &hydration_pool;
                let mapper = move |result| {
                    // Mapper entry proves nonempty mutation dispatch. The
                    // exclusive worker GIL keeps the marker unobservable until
                    // success/error/panic cleanup has made the state terminal.
                    if let Some(marker) = &mapper_marker {
                        marker.store(true, Ordering::Release);
                    }
                    materialize_projected_batch(
                        py,
                        mapper_package,
                        mapper_slots,
                        facades,
                        result,
                        mapper_error,
                        mapper_pool,
                    )
                    .map(BatchMappedOutput::Publication)
                };
                let executor = ProjectedBatchExecutor::new(package.projection.as_ref());
                let nonempty = !batch.is_empty();
                let result = match (&database, &transaction) {
                    (Some(database), None) => {
                        executor
                            .execute_mapped(database.as_ref(), &batch, control, empty, mapper)
                            .await
                    }
                    (None, Some(transaction)) => {
                        let prior_state = if nonempty {
                            Some(transaction.lifecycle_state().await)
                        } else {
                            None
                        };
                        let result = executor
                            .execute_in_transaction_mapped(
                                transaction,
                                &batch,
                                control,
                                empty,
                                mapper,
                            )
                            .await;
                        let mark =
                            if !nonempty || prior_state != Some(TransactionContextState::Active) {
                                false
                            } else if result.is_ok() {
                                true
                            } else {
                                transaction.lifecycle_state().await
                                    == TransactionContextState::RollbackOnly
                            };
                        if mark && let Some(marker) = &successor_batch_marker {
                            marker.store(true, Ordering::Release);
                        }
                        result
                    }
                    _ => Err(SdkExecutionDiagnostic::internal_failure()),
                };
                let output = match result {
                    Ok(mapped) => Ok(mapped.finish()),
                    Err(diagnostic) => match take_binding_materialization_error(&binding_error) {
                        Some(error) => Err(error),
                        None => Err(py_sdk_diagnostic(diagnostic)),
                    },
                };
                // This outer owner survives mapper-capture unwind and common
                // rollback/poison. Any unpublished generated objects are
                // released only now, under this worker's still-held GIL.
                drop(hydration_pool);
                output
            })
        })
    }

    fn delete_many_projected(&self, py: Python<'_>, iids: Vec<String>) -> PyResult<()> {
        ProjectedBatch::validate_binding_row_count(iids.len()).map_err(py_sdk_diagnostic)?;
        let control = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        );
        let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
            self.package.projection.as_ref(),
            self.type_id.clone(),
            ProjectedBatchOperation::Delete,
            &control,
        )
        .map_err(py_sdk_diagnostic)?;
        let mut rows = reserved_binding_rows(iids.len()).map_err(py_sdk_diagnostic)?;
        for iid in iids {
            let row = ProjectedBatchRow::Delete { iid };
            budget
                .try_add_row(self.package.projection.as_ref(), &row)
                .map_err(py_sdk_diagnostic)?;
            rows.push(row);
        }
        drop(budget);
        let batch = ProjectedBatch::try_new_for_invocation(
            self.package.projection.as_ref(),
            self.type_id.clone(),
            ProjectedBatchOperation::Delete,
            rows,
            &control,
        )
        .map_err(py_sdk_diagnostic)?;
        if !batch.is_empty()
            && let Some(transaction) = &self.transaction
        {
            let transaction = transaction.clone();
            let runtime = Arc::clone(&self.runtime);
            let package = Arc::clone(&self.package);
            let marker = self.successor_batch_marker.clone();
            return provider_block_on_with_gil(py, runtime.as_ref(), move |_py| {
                Box::pin(async move {
                    let prior_state = transaction.lifecycle_state().await;
                    let executor = ProjectedBatchExecutor::new(package.projection.as_ref());
                    let result = executor
                        .execute_in_transaction_mapped(
                            &transaction,
                            &batch,
                            control,
                            (),
                            |result| match result {
                                ProjectedBatchResult::Deleted => Ok(()),
                                ProjectedBatchResult::Things(_) => {
                                    Err(SdkExecutionDiagnostic::internal_failure())
                                }
                                _ => Err(SdkExecutionDiagnostic::internal_failure()),
                            },
                        )
                        .await;
                    let mark = prior_state == TransactionContextState::Active
                        && (result.is_ok()
                            || transaction.lifecycle_state().await
                                == TransactionContextState::RollbackOnly);
                    if mark && let Some(marker) = marker {
                        marker.store(true, Ordering::Release);
                    }
                    result.map_err(py_sdk_diagnostic)
                })
            });
        }
        self.execute_projected_batch(py, &batch, control, (), |result| match result {
            ProjectedBatchResult::Deleted => Ok(()),
            ProjectedBatchResult::Things(_) => Err(SdkExecutionDiagnostic::internal_failure()),
            _ => Err(SdkExecutionDiagnostic::internal_failure()),
        })
        .map_err(py_sdk_diagnostic)
    }

    fn execute_projected_batch<T, F>(
        &self,
        py: Python<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        empty: T,
        mapper: F,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        T: Send,
        F: FnOnce(ProjectedBatchResult) -> Result<T, SdkExecutionDiagnostic> + Send,
    {
        let executor = ProjectedBatchExecutor::new(self.package.projection.as_ref());
        match (&self.database, &self.transaction) {
            (Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.execute_mapped(database.as_ref(), batch, control, empty, mapper),
            ),
            (None, Some(transaction)) => {
                let prior_state = (!batch.is_empty()).then(|| {
                    provider_block_on(py, self.runtime.as_ref(), transaction.lifecycle_state())
                });
                let result = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    executor.execute_in_transaction_mapped(
                        transaction,
                        batch,
                        control,
                        empty,
                        mapper,
                    ),
                );
                let mark =
                    if batch.is_empty() || prior_state != Some(TransactionContextState::Active) {
                        false
                    } else if result.is_ok() {
                        true
                    } else {
                        provider_block_on(py, self.runtime.as_ref(), transaction.lifecycle_state())
                            == TransactionContextState::RollbackOnly
                    };
                if mark && let Some(marker) = &self.successor_batch_marker {
                    marker.store(true, Ordering::Release);
                }
                result
            }
            _ => Err(SdkExecutionDiagnostic::internal_failure()),
        }
    }

    fn descriptor(&self) -> PyResult<TypeDescriptor> {
        self.package
            .projection
            .descriptor(&self.type_id)
            .cloned()
            .map_err(py_orm_error)
    }

    fn ensure_instance(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let expected = self
            .package
            .class(&self.type_id, ProjectedModelForm::Complete)?;
        if value.get_type().as_ptr() != expected.bind(py).as_ptr() {
            return Err(py_type_error(
                "insert requires an instance of the manager's exact registered class",
            ));
        }
        Ok(())
    }

    fn entity_manager(
        &self,
        descriptor: Arc<EntityDescriptor>,
    ) -> PyResult<DynamicEntityManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicEntityManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicEntityManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
    }

    fn relation_manager(
        &self,
        descriptor: Arc<RelationDescriptor>,
    ) -> PyResult<DynamicRelationManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicRelationManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicRelationManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
    }
}

fn ensure_batch_instance_at(
    py: Python<'_>,
    package: &InstalledPackage,
    type_id: &TypeId,
    value: &Bound<'_, PyAny>,
    ordinal: usize,
) -> PyResult<()> {
    let expected = package.class(type_id, ProjectedModelForm::Complete)?;
    if value.get_type().as_ptr() != expected.bind(py).as_ptr() {
        return Err(py_sdk_diagnostic(generated_token_package_mismatch_at([
            SdkDiagnosticPathSegment::Argument(
                SdkDiagnosticName::new("rows").expect("the fixed batch argument is canonical"),
            ),
            SdkDiagnosticPathSegment::Index(projected_index(ordinal)),
            SdkDiagnosticPathSegment::Type(type_id.clone()),
        ])));
    }
    Ok(())
}

fn bounded_projected_facades(values: &Bound<'_, PyAny>) -> PyResult<Vec<Py<PyAny>>> {
    if !supports_python_iteration(values) {
        return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
            "batch_rows_not_iterable",
            "Successor batch rows must be an iterable of exact generated model objects",
            &projected_batch_rows_path(),
        )));
    }
    let mut facades = Vec::new();
    let iterator = values.try_iter()?;
    for (ordinal, value) in iterator.enumerate() {
        let value = value?;
        ProjectedBatch::validate_binding_row_count(ordinal.saturating_add(1))
            .map_err(py_sdk_diagnostic)?;
        facades
            .try_reserve(1)
            .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        facades.push(value.unbind());
    }
    Ok(facades)
}

fn bounded_projected_iids(values: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    if !supports_python_iteration(values) {
        return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
            "batch_rows_not_iterable",
            "Successor delete rows must be an iterable of exact strings",
            &projected_batch_rows_path(),
        )));
    }
    let mut iids = Vec::new();
    let iterator = values.try_iter()?;
    for (ordinal, value) in iterator.enumerate() {
        let value = value?;
        ProjectedBatch::validate_binding_row_count(ordinal.saturating_add(1))
            .map_err(py_sdk_diagnostic)?;
        let path = projected_batch_iid_path(ordinal);
        let value = value.cast_exact::<PyString>().map_err(|_| {
            py_sdk_diagnostic(batch_input_shape_diagnostic(
                "batch_iid_type_mismatch",
                "Successor delete rows require exact string IIDs",
                &path,
            ))
        })?;
        let text = value.to_str().map_err(|_| {
            py_sdk_diagnostic(batch_input_shape_diagnostic(
                "batch_iid_type_mismatch",
                "Successor delete rows require UTF-8 string IIDs",
                &path,
            ))
        })?;
        let iid = copy_canonical_batch_iid(text, ordinal)?;
        iids.try_reserve(1)
            .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        iids.push(iid);
    }
    Ok(iids)
}

fn supports_python_iteration(value: &Bound<'_, PyAny>) -> bool {
    let class = value.get_type();
    !type_slot(&class, pyo3::ffi::Py_tp_iter).is_null()
        // SAFETY: the value is live under the GIL. PySequence_Check is a
        // non-callbacking type-slot predicate used by PyObject_GetIter itself.
        || unsafe { pyo3::ffi::PySequence_Check(value.as_ptr()) } != 0
}

fn projected_batch_rows_path() -> [SdkDiagnosticPathSegment; 1] {
    [SdkDiagnosticPathSegment::Argument(
        SdkDiagnosticName::new("rows").expect("the fixed batch argument is canonical"),
    )]
}

fn reserved_binding_rows<T>(capacity: usize) -> Result<Vec<T>, SdkExecutionDiagnostic> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(capacity)
        .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
    Ok(rows)
}

fn reject_duplicate_batch_facades(identities: &mut [(usize, usize)]) -> PyResult<()> {
    identities.sort_unstable();
    let mut conflict: Option<(usize, usize)> = None;
    for pair in identities.windows(2) {
        let [(left_pointer, left_ordinal), (right_pointer, right_ordinal)] = pair else {
            continue;
        };
        if left_pointer == right_pointer {
            let candidate = (
                (*left_ordinal).min(*right_ordinal),
                (*left_ordinal).max(*right_ordinal),
            );
            if conflict.is_none_or(|current| (candidate.1, candidate.0) < (current.1, current.0)) {
                conflict = Some(candidate);
            }
        }
    }
    let Some((first, duplicate)) = conflict else {
        return Ok(());
    };
    let diagnostic = SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new("duplicate_batch_input_identity")
            .expect("the static duplicate-facade code is canonical"),
        SdkDiagnosticMessage::new(
            "The same generated Python model object appears more than once in the batch",
        )
        .expect("the static duplicate-facade message is canonical"),
    )
    .try_at(SdkDiagnosticPathSegment::Argument(
        SdkDiagnosticName::new("rows").expect("the fixed batch argument is canonical"),
    ))
    .and_then(|diagnostic| {
        diagnostic.try_at(SdkDiagnosticPathSegment::Index(projected_index(duplicate)))
    })
    .and_then(|diagnostic| {
        diagnostic.try_with_detail(
            SdkDiagnosticName::new("first_conflicting_index")
                .expect("the fixed conflict detail is canonical"),
            SdkDiagnosticDetailValue::Count(projected_index(first)),
        )
    })
    .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure());
    Err(py_sdk_diagnostic(diagnostic))
}

fn validate_staged_batch_facades_after_lowering(
    py: Python<'_>,
    slots: &ProjectedFacadeSlots,
    facades: &[StagedBatchFacade],
) -> PyResult<()> {
    for (ordinal, facade) in facades.iter().enumerate() {
        let current = slots.snapshot(py, facade.instance.bind(py)).map_err(|_| {
            py_sdk_diagnostic(batch_input_shape_diagnostic(
                "generated_model_layout_mismatch",
                "The generated model layout changed during successor batch lowering",
                &projected_batch_row_path(ordinal),
            ))
        })?;
        if !current.values.bind(py).is(facade.snapshot.values.bind(py))
            || !current.iid.bind(py).is(facade.snapshot.iid.bind(py))
        {
            return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
                "generated_model_changed_during_batch",
                "A generated model changed during successor batch lowering",
                &projected_batch_row_path(ordinal),
            )));
        }
    }
    Ok(())
}

fn projected_batch_row_path(ordinal: usize) -> [SdkDiagnosticPathSegment; 2] {
    [
        SdkDiagnosticPathSegment::Argument(
            SdkDiagnosticName::new("rows").expect("the fixed batch argument is canonical"),
        ),
        SdkDiagnosticPathSegment::Index(projected_index(ordinal)),
    ]
}

fn projected_batch_iid_path(ordinal: usize) -> [SdkDiagnosticPathSegment; 3] {
    [
        SdkDiagnosticPathSegment::Argument(
            SdkDiagnosticName::new("rows").expect("the fixed batch argument is canonical"),
        ),
        SdkDiagnosticPathSegment::Index(projected_index(ordinal)),
        SdkDiagnosticPathSegment::Argument(
            SdkDiagnosticName::new("iid").expect("the fixed IID argument is canonical"),
        ),
    ]
}

fn batch_input_shape_diagnostic(
    code: &'static str,
    message: &'static str,
    path: &[SdkDiagnosticPathSegment],
) -> SdkExecutionDiagnostic {
    path.iter().cloned().fold(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new(code).expect("the static batch-shape code is canonical"),
            SdkDiagnosticMessage::new(message)
                .expect("the static batch-shape message is canonical"),
        ),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
        },
    )
}

fn add_projected_attribute_pool_objects<'a>(
    total: &mut usize,
    values: impl Iterator<Item = &'a ProjectedAttributeValue>,
) -> Result<(), SdkExecutionDiagnostic> {
    for value in values {
        let named_zone = matches!(
            value.value(),
            CanonicalValue::DateTimeTz(value)
                if matches!(value.zone(), TimeZoneDesignator::Named(_))
        );
        *total = total
            .checked_add(1 + usize::from(named_zone))
            .ok_or_else(ProjectedBatch::binding_allocation_failure)?;
    }
    Ok(())
}

fn projected_batch_hydration_pool_capacity(
    things: &[ProjectedThing],
) -> Result<usize, SdkExecutionDiagnostic> {
    let mut total = 0_usize;
    for thing in things {
        total = total
            .checked_add(1)
            .ok_or_else(ProjectedBatch::binding_allocation_failure)?;
        add_projected_attribute_pool_objects(
            &mut total,
            thing.fields().values().flat_map(|values| values.iter()),
        )?;
        for player in thing.roles().values().flat_map(|players| players.iter()) {
            total = total
                .checked_add(1)
                .ok_or_else(ProjectedBatch::binding_allocation_failure)?;
            match player.exact_form() {
                Some(ProjectedModelForm::Complete) => add_projected_attribute_pool_objects(
                    &mut total,
                    player.fields().values().flat_map(|values| values.iter()),
                )?,
                Some(ProjectedModelForm::Reference) => {
                    add_projected_attribute_pool_objects(&mut total, player.keys().values())?
                }
                None => {}
            }
        }
    }
    Ok(total)
}

fn materialize_projected_batch<'py>(
    py: Python<'py>,
    package: Arc<InstalledPackage>,
    slots: Arc<ProjectedFacadeSlots>,
    facades: Vec<PreparedBatchFacade>,
    result: ProjectedBatchResult,
    binding_error: &Mutex<Option<PyErr>>,
    pool: &BatchHydrationPool,
) -> Result<BatchPublicationGuard<'py>, SdkExecutionDiagnostic> {
    materialize_projected_batch_with_probe(
        py,
        package,
        slots,
        facades,
        result,
        binding_error,
        pool,
        BatchMaterializationProbe::default(),
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct BatchMaterializationProbe {
    fail_output_allocation: bool,
    fail_publication_at: Option<usize>,
    #[cfg(test)]
    panic_publication_at: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn materialize_projected_batch_with_probe<'py>(
    py: Python<'py>,
    package: Arc<InstalledPackage>,
    slots: Arc<ProjectedFacadeSlots>,
    facades: Vec<PreparedBatchFacade>,
    result: ProjectedBatchResult,
    binding_error: &Mutex<Option<PyErr>>,
    pool: &BatchHydrationPool,
    probe: BatchMaterializationProbe,
) -> Result<BatchPublicationGuard<'py>, SdkExecutionDiagnostic> {
    let ProjectedBatchResult::Things(things) = result else {
        return Err(SdkExecutionDiagnostic::internal_failure());
    };
    if things.len() != facades.len() {
        return Err(ProjectedBatchExecutor::binding_materialization_failure(0));
    }
    pool.reserve_exact(projected_batch_hydration_pool_capacity(&things)?)?;

    let nested_origin_capacity = things
        .iter()
        .try_fold(0_usize, |count, thing| {
            thing
                .roles()
                .values()
                .try_fold(count, |count, players| count.checked_add(players.len()))
        })
        .ok_or_else(ProjectedBatch::binding_allocation_failure)?;
    let mut pending_origins = reserved_binding_rows(nested_origin_capacity)?;
    let mut hydrated = reserved_binding_rows(things.len())?;
    for (ordinal, projected) in things.into_iter().enumerate() {
        let projected = Arc::new(projected);
        let value = hydrate_projected_thing_value_staged(
            py,
            package.as_ref(),
            projected.as_ref(),
            Some(&projected),
            &mut pending_origins,
            true,
            Some(pool),
        )
        .map_err(|error| {
            record_binding_materialization_error(binding_error, error);
            ProjectedBatchExecutor::binding_materialization_failure(projected_index(ordinal))
        })?;
        let replacement = slots
            .snapshot_after_fence(py, value.bind(py))
            .map_err(|error| {
                record_binding_materialization_error(binding_error, error);
                ProjectedBatchExecutor::binding_materialization_failure(projected_index(ordinal))
            })?;
        hydrated.push(StagedBatchHydration {
            projected,
            _instance: value,
            replacement,
        });
    }
    pool.verify_fully_consumed()?;

    if probe.fail_output_allocation {
        return Err(ProjectedBatch::binding_allocation_failure());
    }
    let output = match PyList::new(py, facades.iter().map(|facade| facade.instance.bind(py))) {
        Ok(output) => output.into_any().unbind(),
        Err(error) => {
            record_binding_materialization_error(binding_error, error);
            return Err(ProjectedBatchExecutor::binding_materialization_failure(0));
        }
    };
    for (ordinal, facade) in facades.iter().enumerate() {
        let current = slots
            .snapshot_after_fence(py, facade.instance.bind(py))
            .map_err(|_| {
                let error = py_sdk_diagnostic(batch_input_shape_diagnostic(
                    "generated_model_layout_mismatch",
                    "The generated model layout changed while its successor batch was executing",
                    &projected_batch_row_path(ordinal),
                ));
                record_binding_materialization_error(binding_error, error);
                ProjectedBatchExecutor::binding_materialization_failure(projected_index(ordinal))
            })?;
        if !current.values.bind(py).is(facade.snapshot.values.bind(py))
            || !current.iid.bind(py).is(facade.snapshot.iid.bind(py))
        {
            record_binding_materialization_error(
                binding_error,
                py_sdk_diagnostic(batch_input_shape_diagnostic(
                    "generated_model_changed_during_batch",
                    "A generated model changed while its successor batch was executing",
                    &projected_batch_row_path(ordinal),
                )),
            );
            return Err(ProjectedBatchExecutor::binding_materialization_failure(
                projected_index(ordinal),
            ));
        }
    }
    let activation_capacity = nested_origin_capacity
        .checked_add(facades.len())
        .ok_or_else(ProjectedBatch::binding_allocation_failure)?;
    let mut activation_guard =
        PendingActivationGuard::new(package.facade_origins.clone(), activation_capacity)?;
    for origin in pending_origins {
        if let Err(error) = activation_guard.stage(py, origin.origin, origin.proof, None) {
            record_binding_materialization_error(binding_error, error);
            return Err(ProjectedBatchExecutor::binding_materialization_failure(0));
        }
    }
    for (facade, hydrated) in facades.iter().zip(hydrated.iter()) {
        if let Err(error) = activation_guard.stage(
            py,
            facade.origin.clone_ref(py),
            FacadeProjectionProof::Thing(Arc::clone(&hydrated.projected)),
            Some(facade.snapshot.clone_ref(py)),
        ) {
            record_binding_materialization_error(binding_error, error);
            return Err(ProjectedBatchExecutor::binding_materialization_failure(0));
        }
    }
    let activations = activation_guard.into_activations();

    let mut guard = BatchPublicationGuard {
        py,
        package,
        slots,
        facades,
        _hydrated: hydrated,
        output: Some(output),
        activations,
        published: 0,
        armed: true,
    };
    for ordinal in 0..guard.facades.len() {
        #[cfg(test)]
        if probe.panic_publication_at == Some(ordinal) {
            panic!("injected projected batch mapper panic at row {ordinal}");
        }
        if probe.fail_publication_at == Some(ordinal) {
            record_binding_materialization_error(
                binding_error,
                py_runtime_error("injected projected batch publication failure"),
            );
            return Err(ProjectedBatchExecutor::binding_materialization_failure(
                projected_index(ordinal),
            ));
        }
        let facade = &guard.facades[ordinal];
        let replacement = &guard._hydrated[ordinal].replacement;
        let instance = facade.instance.bind(py);
        if let Err(error) = guard
            .slots
            .values
            .set(py, instance, replacement.values.bind(py))
        {
            record_binding_materialization_error(binding_error, error);
            return Err(ProjectedBatchExecutor::binding_materialization_failure(
                projected_index(ordinal),
            ));
        }
        if let Err(error) = guard.slots.iid.set(py, instance, replacement.iid.bind(py)) {
            guard
                .slots
                .values
                .set_infallible(py, instance, facade.snapshot.values.bind(py));
            record_binding_materialization_error(binding_error, error);
            return Err(ProjectedBatchExecutor::binding_materialization_failure(
                projected_index(ordinal),
            ));
        }
        guard.published = ordinal.saturating_add(1);
    }
    Ok(guard)
}

fn record_binding_materialization_error(binding_error: &Mutex<Option<PyErr>>, error: PyErr) {
    let mut retained = binding_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if retained.is_none() {
        *retained = Some(error);
    }
}

fn take_binding_materialization_error(binding_error: &Mutex<Option<PyErr>>) -> Option<PyErr> {
    binding_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

fn fatal_batch_invariant(message: &std::ffi::CStr) -> ! {
    // SAFETY: the message is a process-lifetime C string. Reaching this path
    // means a Stable-ABI member slot changed applicability while one worker
    // held the GIL continuously after the final pre-I/O validation.
    unsafe { pyo3::ffi::Py_FatalError(message.as_ptr()) }
}

fn exact_type_mro_attribute(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    name: &'static str,
) -> PyResult<Py<PyAny>> {
    // SAFETY: the live class is GIL-bound. Requiring the exact built-in `type`
    // metaclass makes `__mro__` and each `__dict__` lookup non-overridable.
    if unsafe {
        pyo3::ffi::Py_IS_TYPE(
            class.as_ptr(),
            std::ptr::addr_of_mut!(pyo3::ffi::PyType_Type),
        )
    } == 0
    {
        return Err(py_type_error(
            "successor generated classes require the exact built-in type metaclass",
        ));
    }
    let mro = py_getattr_cstr(py, class.as_any(), pyo3::ffi::c_str!("__mro__"))?
        .into_bound(py)
        .cast_into::<PyTuple>()
        .map_err(|_| py_type_error("successor generated class MRO is not an exact tuple"))?;
    for base in mro.iter() {
        let base = base.cast_into::<PyType>().map_err(|_| {
            py_type_error("successor generated class MRO contains a non-type entry")
        })?;
        // SAFETY: the live MRO entry is GIL-bound.
        if unsafe {
            pyo3::ffi::Py_IS_TYPE(
                base.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyType_Type),
            )
        } == 0
        {
            return Err(py_type_error(
                "successor generated class MRO uses a custom metaclass",
            ));
        }
        let dictionary = py_getattr_cstr(py, base.as_any(), pyo3::ffi::c_str!("__dict__"))?;
        // SAFETY: the exact built-in `type` data descriptor returned the
        // class's live mappingproxy. PyMapping_Items is Stable ABI and yields
        // raw key/value pairs without looking up the projected descriptor.
        let items = unsafe { pyo3::ffi::PyMapping_Items(dictionary.bind(py).as_ptr()) };
        // SAFETY: a non-null result is one owned list reference.
        let items = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, items) }?
            .cast_into::<PyList>()
            .map_err(|_| py_type_error("successor generated class dictionary items are invalid"))?;
        let mut found = None;
        for item in items.iter() {
            let item = item.cast_into_exact::<PyTuple>().map_err(|_| {
                py_type_error("successor generated class dictionary contains an invalid item")
            })?;
            if item.len() != 2 {
                return Err(py_type_error(
                    "successor generated class dictionary contains an invalid item",
                ));
            }
            let key = item.get_item(0)?;
            let key = key.cast_into_exact::<PyString>().map_err(|_| {
                py_type_error("successor generated class dictionaries require exact string keys")
            })?;
            if key.to_str()? == name {
                found = Some(item.get_item(1)?.unbind());
            }
        }
        if let Some(found) = found {
            return Ok(found);
        }
    }
    Err(py_type_error(format!(
        "successor generated class MRO does not define {name}"
    )))
}

fn py_getattr_cstr(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    name: &std::ffi::CStr,
) -> PyResult<Py<PyAny>> {
    // SAFETY: value and the process-lifetime C string are valid under the GIL.
    let result = unsafe { pyo3::ffi::PyObject_GetAttrString(value.as_ptr(), name.as_ptr()) };
    // SAFETY: a non-null result is one owned reference; null preserves PyErr.
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, result) }.map(Bound::unbind)
}

impl ProjectedFacadeSlot {
    fn capture(py: Python<'_>, class: &Bound<'_, PyType>, name: &'static str) -> PyResult<Self> {
        let descriptor = exact_type_mro_attribute(py, class, name)?.into_bound(py);
        // SAFETY: both pointers are live while the GIL is held. The static
        // member-descriptor type, PyType_GetSlot, and these two slot IDs are
        // part of the CPython Stable ABI supported by abi3-py312.
        if unsafe {
            pyo3::ffi::Py_IS_TYPE(
                descriptor.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyMemberDescr_Type),
            )
        } == 0
        {
            return Err(py_type_error(format!(
                "successor model {name} storage is not a canonical member descriptor"
            )));
        }
        let descriptor_name: String =
            py_getattr_cstr(py, &descriptor, pyo3::ffi::c_str!("__name__"))?
                .bind(py)
                .extract()?;
        if descriptor_name != name {
            return Err(py_type_error(format!(
                "successor model member descriptor has the wrong name for {name}"
            )));
        }
        let owner = py_getattr_cstr(py, &descriptor, pyo3::ffi::c_str!("__objclass__"))?
            .into_bound(py)
            .cast_into::<PyType>()
            .map_err(|_| py_type_error("successor model member descriptor owner is not a type"))?;
        // SAFETY: class and owner are live exact PyType objects under the GIL.
        if unsafe {
            pyo3::ffi::PyType_IsSubtype(
                class.as_ptr().cast(),
                owner.as_ptr().cast::<pyo3::ffi::PyTypeObject>(),
            )
        } == 0
        {
            return Err(py_type_error(format!(
                "successor model class does not carry the canonical {name} slot"
            )));
        }
        let descriptor_type = descriptor.get_type();
        // SAFETY: the exact descriptor type is live under the GIL and the
        // queried slots are Stable-ABI type slots.
        let getter = unsafe {
            pyo3::ffi::PyType_GetSlot(descriptor_type.as_ptr().cast(), pyo3::ffi::Py_tp_descr_get)
        };
        // SAFETY: same justification as for the getter.
        let setter = unsafe {
            pyo3::ffi::PyType_GetSlot(descriptor_type.as_ptr().cast(), pyo3::ffi::Py_tp_descr_set)
        };
        if getter.is_null() || setter.is_null() {
            return Err(py_type_error(
                "successor model member descriptor lacks stable get/set slots",
            ));
        }
        // SAFETY: PyType_GetSlot returned the function pointer associated with
        // the exact slot ID and the Stable-ABI function signature.
        let getter = unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, pyo3::ffi::descrgetfunc>(getter)
        };
        // SAFETY: same, for Py_tp_descr_set.
        let setter = unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, pyo3::ffi::descrsetfunc>(setter)
        };
        Ok(Self {
            name,
            descriptor: descriptor.unbind(),
            owner: owner.unbind(),
            getter,
            setter,
        })
    }

    fn validate_owner(&self, py: Python<'_>, class: &Bound<'_, PyType>) -> PyResult<()> {
        // SAFETY: class and the retained owner are live PyType objects under
        // the uninterrupted worker GIL.
        if unsafe {
            pyo3::ffi::PyType_IsSubtype(class.as_ptr().cast(), self.owner.bind(py).as_ptr().cast())
        } == 0
        {
            return Err(py_type_error(
                "successor model slot owner is no longer in the exact class MRO",
            ));
        }
        if !exact_type_mro_attribute(py, class, self.name)?
            .bind(py)
            .is(self.descriptor.bind(py))
        {
            return Err(py_type_error(
                "successor model effective member descriptor changed after installation",
            ));
        }
        Ok(())
    }

    fn get(&self, py: Python<'_>, instance: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let value = unsafe {
            (self.getter)(
                self.descriptor.bind(py).as_ptr(),
                instance.as_ptr(),
                instance.get_type().as_ptr(),
            )
        };
        if value.is_null() {
            Err(PyErr::fetch(py))
        } else {
            // SAFETY: a non-null descriptor-get result is one owned reference.
            Ok(unsafe { Bound::<PyAny>::from_owned_ptr(py, value).unbind() })
        }
    }

    fn set(
        &self,
        py: Python<'_>,
        instance: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        // SAFETY: the retained exact descriptor, compatible facade, and value
        // are alive under the GIL. Final validation established applicability.
        let status = unsafe {
            (self.setter)(
                self.descriptor.bind(py).as_ptr(),
                instance.as_ptr(),
                value.as_ptr(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(PyErr::fetch(py))
        }
    }

    fn set_infallible(
        &self,
        py: Python<'_>,
        instance: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) {
        if self.set(py, instance, value).is_err() {
            fatal_batch_invariant(pyo3::ffi::c_str!(
                "validated projected facade member assignment failed"
            ));
        }
    }
}

impl ProjectedFacadeSlots {
    fn capture(
        py: Python<'_>,
        class: &Py<PyType>,
        attribute_value: bool,
        allocator: Arc<ProjectedHeapAllocator>,
    ) -> PyResult<Self> {
        let class = class.bind(py);
        let trusted_mro = exact_type_mro(py, class)?;
        validate_projected_generic_layout(py, class, allocator.as_ref(), &trusted_mro)?;
        let slots = Self {
            class: class.clone().unbind(),
            values: ProjectedFacadeSlot::capture(py, class, "_values")?,
            iid: ProjectedFacadeSlot::capture(py, class, "_iid")?,
            attribute_value: attribute_value
                .then(|| ProjectedFacadeSlot::capture(py, class, "_attribute_value"))
                .transpose()?,
            allocator,
            trusted_mro,
        };
        slots.validate_layout(py)?;
        slots.probe_storage(py)?;
        Ok(slots)
    }

    fn validate_layout(&self, py: Python<'_>) -> PyResult<()> {
        let class = self.class.bind(py);
        validate_projected_generic_layout(py, class, self.allocator.as_ref(), &self.trusted_mro)?;
        self.values.validate_owner(py, class)?;
        self.iid.validate_owner(py, class)?;
        if let Some(attribute_value) = &self.attribute_value {
            attribute_value.validate_owner(py, class)?;
        }
        Ok(())
    }

    fn probe_storage(&self, py: Python<'_>) -> PyResult<()> {
        let mut members = vec![&self.values, &self.iid];
        if let Some(attribute_value) = &self.attribute_value {
            members.push(attribute_value);
        }
        probe_projected_object_members(
            py,
            self.class.bind(py),
            self.allocator.object_new.bind(py),
            &members,
        )
    }

    fn snapshot(
        &self,
        py: Python<'_>,
        instance: &Bound<'_, PyAny>,
    ) -> PyResult<ProjectedFacadeSnapshot> {
        let class = self.class.bind(py);
        if instance.get_type().as_ptr() != class.as_ptr() {
            return Err(py_type_error(
                "successor batch requires the manager's exact registered class",
            ));
        }
        self.validate_layout(py)?;
        Ok(ProjectedFacadeSnapshot {
            iid: self.iid.get(py, instance)?,
            values: self.values.get(py, instance)?,
        })
    }

    fn snapshot_after_fence(
        &self,
        py: Python<'_>,
        instance: &Bound<'_, PyAny>,
    ) -> PyResult<ProjectedFacadeSnapshot> {
        if instance.get_type().as_ptr() != self.class.bind(py).as_ptr() {
            return Err(py_type_error(
                "successor batch facade class changed after its pre-provider layout fence",
            ));
        }
        Ok(ProjectedFacadeSnapshot {
            iid: self.iid.get(py, instance)?,
            values: self.values.get(py, instance)?,
        })
    }

    fn allocate_fresh<'py>(
        &self,
        py: Python<'py>,
        pool: &BatchHydrationPool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let class = self.class.bind(py);
        // The full static-MRO/allocator/finalizer fence ran immediately before
        // provider polling while this worker already held the uninterrupted
        // GIL. Mapper hydration contains no Python callbacks, so the retained
        // exact heap type and its layout cannot change before this call.
        let instance = self.allocator.object_new.bind(py).call1((class,))?;
        pool.retain_fresh(&instance, class)?;
        Ok(instance)
    }

    fn allocate_initialized_after_fence(
        &self,
        py: Python<'_>,
        values: &Bound<'_, PyAny>,
        iid: &Bound<'_, PyAny>,
        attribute_value: Option<&Bound<'_, PyAny>>,
        pool: &BatchHydrationPool,
    ) -> PyResult<Py<PyAny>> {
        if self.attribute_value.is_some() != attribute_value.is_some() {
            return Err(py_runtime_error(
                "successor generated hydration slot form was inconsistent",
            ));
        }
        // Every fallible replacement object is fully built before the exact
        // object allocation. The remaining T_OBJECT_EX writes cannot allocate
        // or invoke Python while all displaced values remain retained.
        let instance = self.allocate_fresh(py, pool)?;
        self.values.set_infallible(py, &instance, values);
        self.iid.set_infallible(py, &instance, iid);
        if let (Some(slot), Some(value)) = (&self.attribute_value, attribute_value) {
            slot.set_infallible(py, &instance, value);
        }
        Ok(instance.unbind())
    }

    fn replace_infallible(
        &self,
        py: Python<'_>,
        instance: &Bound<'_, PyAny>,
        replacement: &ProjectedFacadeSnapshot,
    ) {
        self.values
            .set_infallible(py, instance, replacement.values.bind(py));
        self.iid
            .set_infallible(py, instance, replacement.iid.bind(py));
    }
}

const PROJECTED_LAYOUT_FLAG_MASK: std::ffi::c_ulong = pyo3::ffi::Py_TPFLAGS_HEAPTYPE
    | pyo3::ffi::Py_TPFLAGS_HAVE_GC
    | pyo3::ffi::Py_TPFLAGS_IS_ABSTRACT
    | pyo3::ffi::Py_TPFLAGS_IMMUTABLETYPE
    | pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END;

fn exact_type_layout_int(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    name: &std::ffi::CStr,
) -> PyResult<isize> {
    py_getattr_cstr(py, class.as_any(), name)?
        .bind(py)
        .extract()
}

fn exact_type_mro(py: Python<'_>, class: &Bound<'_, PyType>) -> PyResult<Vec<ProjectedTypeLayout>> {
    if unsafe {
        pyo3::ffi::Py_IS_TYPE(
            class.as_ptr(),
            std::ptr::addr_of_mut!(pyo3::ffi::PyType_Type),
        )
    } == 0
    {
        return Err(py_type_error(
            "successor generated classes require the exact built-in type metaclass",
        ));
    }
    let mro = py_getattr_cstr(py, class.as_any(), pyo3::ffi::c_str!("__mro__"))?
        .into_bound(py)
        .cast_into_exact::<PyTuple>()
        .map_err(|_| py_type_error("successor generated class MRO is not an exact tuple"))?;
    let mut retained = Vec::new();
    retained
        .try_reserve_exact(mro.len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    for base in mro.iter() {
        let base = base.cast_into::<PyType>().map_err(|_| {
            py_type_error("successor generated class MRO contains a non-type entry")
        })?;
        if unsafe {
            pyo3::ffi::Py_IS_TYPE(
                base.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyType_Type),
            )
        } == 0
        {
            return Err(py_type_error(
                "successor generated class MRO uses a custom metaclass",
            ));
        }
        let flags = unsafe { pyo3::ffi::PyType_GetFlags(base.as_ptr().cast()) }
            & PROJECTED_LAYOUT_FLAG_MASK;
        let basicsize = exact_type_layout_int(py, &base, pyo3::ffi::c_str!("__basicsize__"))?;
        let itemsize = exact_type_layout_int(py, &base, pyo3::ffi::c_str!("__itemsize__"))?;
        let dictoffset = exact_type_layout_int(py, &base, pyo3::ffi::c_str!("__dictoffset__"))?;
        let weakrefoffset =
            exact_type_layout_int(py, &base, pyo3::ffi::c_str!("__weakrefoffset__"))?;
        retained.push(ProjectedTypeLayout {
            class: base.unbind(),
            flags,
            basicsize,
            itemsize,
            dictoffset,
            weakrefoffset,
        });
    }
    Ok(retained)
}

fn validate_exact_type_mro(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    trusted_mro: &[ProjectedTypeLayout],
) -> PyResult<()> {
    let current = exact_type_mro(py, class)?;
    if current.len() != trusted_mro.len()
        || current.iter().zip(trusted_mro).any(|(current, trusted)| {
            !current.class.bind(py).is(trusted.class.bind(py))
                || current.flags != trusted.flags
                || current.basicsize != trusted.basicsize
                || current.itemsize != trusted.itemsize
                || current.dictoffset != trusted.dictoffset
                || current.weakrefoffset != trusted.weakrefoffset
        })
    {
        return Err(py_type_error(
            "successor generated class MRO changed after projection installation",
        ));
    }
    Ok(())
}

fn type_slot(class: &Bound<'_, PyType>, slot: i32) -> *mut std::ffi::c_void {
    // SAFETY: class is a live exact type under the GIL and callers use only
    // Stable-ABI slot identifiers supported by the abi3-py312 floor.
    unsafe { pyo3::ffi::PyType_GetSlot(class.as_ptr().cast(), slot) }
}

fn probe_projected_object_members(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    constructor: &Bound<'_, PyAny>,
    members: &[&ProjectedFacadeSlot],
) -> PyResult<()> {
    let instance = constructor.call1((class,))?;
    // SAFETY: exact pointer identity under the GIL without creating an owned
    // type wrapper during this installation-only behavior probe.
    if unsafe { pyo3::ffi::Py_TYPE(instance.as_ptr()) } != class.as_ptr().cast() {
        return Err(py_type_error(
            "trusted successor constructor returned the wrong exact class",
        ));
    }
    let mut sentinels = Vec::new();
    sentinels
        .try_reserve_exact(members.len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    for member in members {
        let sentinel = fallible_python_dict(py)?.into_any().unbind();
        member.set(py, &instance, sentinel.bind(py))?;
        sentinels.push(sentinel);
    }
    for (member, sentinel) in members.iter().zip(&sentinels) {
        let observed = member.get(py, &instance)?;
        if !observed.bind(py).is(sentinel.bind(py)) {
            return Err(py_type_error(
                "successor generated member descriptors do not expose distinct writable object slots",
            ));
        }
    }
    Ok(())
}

fn validate_projected_generic_layout(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    trusted: &ProjectedHeapAllocator,
    trusted_mro: &[ProjectedTypeLayout],
) -> PyResult<()> {
    validate_exact_type_mro(py, class, trusted_mro)?;
    if trusted_mro.last().is_none_or(|base| {
        base.class.bind(py).as_ptr()
            != std::ptr::addr_of_mut!(pyo3::ffi::PyBaseObject_Type).cast::<pyo3::ffi::PyObject>()
    }) {
        return Err(py_type_error(
            "successor generated classes must retain the exact object-rooted MRO",
        ));
    }
    for (index, base) in trusted_mro.iter().enumerate() {
        let layout = base;
        let base = layout.class.bind(py);
        if base.as_ptr()
            == std::ptr::addr_of_mut!(pyo3::ffi::PyBaseObject_Type).cast::<pyo3::ffi::PyObject>()
        {
            continue;
        }
        let flags = unsafe { pyo3::ffi::PyType_GetFlags(base.as_ptr().cast()) };
        if flags & pyo3::ffi::Py_TPFLAGS_HEAPTYPE == 0
            || flags & pyo3::ffi::Py_TPFLAGS_HAVE_GC == 0
            || (index == 0 && flags & pyo3::ffi::Py_TPFLAGS_IS_ABSTRACT != 0)
            || flags & pyo3::ffi::Py_TPFLAGS_IMMUTABLETYPE != 0
            || flags & pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END != 0
            || layout.itemsize != 0
            || layout.dictoffset != 0
        {
            return Err(py_type_error(
                "successor generated classes require an exact concrete Python heap MRO",
            ));
        }
        let generic_allocator =
            pyo3::ffi::PyType_GenericAlloc as *const () as *mut std::ffi::c_void;
        if type_slot(base, pyo3::ffi::Py_tp_alloc) != generic_allocator {
            return Err(py_type_error(
                "successor generated classes require the canonical generic allocator",
            ));
        }
        if type_slot(base, pyo3::ffi::Py_tp_dealloc) as usize != trusted.trusted_deallocator {
            return Err(py_type_error(
                "successor generated classes require the trusted Python heap deallocator",
            ));
        }
        if type_slot(base, pyo3::ffi::Py_tp_new) as usize != trusted.trusted_constructor {
            return Err(py_type_error(
                "successor generated classes require the exact object constructor family",
            ));
        }
        if type_slot(base, pyo3::ffi::Py_tp_free) as usize != trusted.trusted_free
            || type_slot(base, pyo3::ffi::Py_tp_traverse) as usize != trusted.trusted_traverse
            || type_slot(base, pyo3::ffi::Py_tp_clear) as usize != trusted.trusted_clear
            || type_slot(base, pyo3::ffi::Py_tp_is_gc) as usize != trusted.trusted_is_gc
            || flags & (pyo3::ffi::Py_TPFLAGS_HAVE_GC | pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END)
                != trusted.trusted_gc_flags
            || layout.basicsize <= 0
        {
            return Err(py_type_error(
                "successor generated classes left the trusted Python heap lifecycle family",
            ));
        }
        if !type_slot(base, pyo3::ffi::Py_tp_finalize).is_null()
            || !type_slot(base, pyo3::ffi::Py_tp_del).is_null()
        {
            return Err(py_type_error(
                "successor generated classes cannot define Python finalizers",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn install_projection(
    py: Python<'_>,
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
    schema_authority: Option<&[u8]>,
) -> PyResult<Arc<InstalledPackage>> {
    install_projection_with_structs(
        py,
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
        Vec::new(),
        schema_authority,
    )
}

fn install_projection_with_structs(
    py: Python<'_>,
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
    structs: Vec<Py<PyType>>,
    schema_authority: Option<&[u8]>,
) -> PyResult<Arc<InstalledPackage>> {
    let (runtime, managed_scope_id, declared_schema_identity) = match schema_authority {
        Some(schema_authority) => {
            let (runtime, scope, declared) = install_authority_backed_projection(
                projection_json,
                semantic_fingerprint_json,
                projection_fingerprint_json,
                schema_authority,
            )?;
            (runtime, Some(scope), Some(declared))
        }
        None => {
            let runtime = decode_runtime_projection_verified(
                projection_json.as_bytes(),
                semantic_fingerprint_json.as_bytes(),
                projection_fingerprint_json.as_bytes(),
            )
            .map_err(py_diagnostic)?;
            if runtime.target() != BindingTarget::Python {
                return Err(py_runtime_error(
                    "runtime projection does not target Python",
                ));
            }
            if projection_uses_ordered_collections(&runtime) {
                return Err(projection_evidence_mismatch());
            }
            verify_legacy_python_projection_evidence(&runtime)?;
            (runtime, None, None)
        }
    };
    let mut expected = BTreeMap::new();
    let mut types_by_label = BTreeMap::new();
    for (id, model) in runtime.models() {
        expected.insert(
            canonical_id(id)?,
            (
                id.clone(),
                model.target_name().as_str().to_owned(),
                model
                    .reference_read()
                    .target_name()
                    .map(|name| name.as_str().to_owned()),
            ),
        );
        if types_by_label
            .insert(id.label().as_str().to_owned(), id.clone())
            .is_some()
        {
            return Err(py_runtime_error(
                "projection contains duplicate type labels",
            ));
        }
    }
    if models.len() != expected.len() {
        return Err(py_value_error(format!(
            "projection requires exactly {} model registrations, received {}",
            expected.len(),
            models.len()
        )));
    }
    let ordered_projection = projection_uses_ordered_collections(&runtime);
    let successor_batch = runtime.generator_handlers()
        == [type_bridge_contract::projection::ProjectionHandler::python_v2()];
    let trusted_type_slots = successor_batch
        .then(|| trusted_python_heap_type_slots(py))
        .transpose()?;
    let mut registered = BTreeMap::new();
    let mut pointers = BTreeSet::new();
    for (complete, reference) in models {
        let id_text: String = complete.bind(py).getattr("__type_id__")?.extract()?;
        let (id, complete_name, reference_name) = expected.remove(&id_text).ok_or_else(|| {
            py_value_error("registered model has an unknown or duplicate __type_id__")
        })?;
        verify_class(py, &complete, &complete_name, "complete", &mut pointers)?;
        match (&reference_name, &reference) {
            (Some(expected_name), Some(class)) => {
                verify_class(py, class, expected_name, "reference", &mut pointers)?;
            }
            (None, None) => {}
            (Some(_), None) => return Err(py_value_error("projection reference class is missing")),
            (None, Some(_)) => return Err(py_value_error("unexpected projection reference class")),
        }
        let batch_slots = successor_batch
            .then(|| {
                ProjectedFacadeSlots::capture(
                    py,
                    &complete,
                    id.kind() == TypeKind::Attribute,
                    Arc::clone(
                        trusted_type_slots
                            .as_ref()
                            .expect("successor installation captured trusted type slots"),
                    ),
                )
            })
            .transpose()?
            .map(Arc::new);
        let batch_reference_slots = successor_batch
            .then(|| {
                reference
                    .as_ref()
                    .map(|reference| {
                        ProjectedFacadeSlots::capture(
                            py,
                            reference,
                            false,
                            Arc::clone(
                                trusted_type_slots
                                    .as_ref()
                                    .expect("successor installation captured trusted type slots"),
                            ),
                        )
                    })
                    .transpose()
            })
            .transpose()?
            .flatten()
            .map(Arc::new);
        registered.insert(
            id,
            RegisteredModel {
                complete,
                reference,
                batch_slots,
                batch_reference_slots,
            },
        );
    }
    if !expected.is_empty() {
        return Err(py_value_error(
            "projection model registration coverage is incomplete",
        ));
    }
    let mut expected_structs = BTreeMap::new();
    for structure in runtime.structs().values() {
        let id = TypeId::new(TypeKind::Struct, structure.id().label().as_str())
            .map_err(py_diagnostic)?;
        expected_structs.insert(
            canonical_id(&id)?,
            (id, structure.target_name().as_str().to_owned()),
        );
    }
    if structs.len() != expected_structs.len() {
        return Err(py_value_error(format!(
            "projection requires exactly {} struct registrations, received {}",
            expected_structs.len(),
            structs.len()
        )));
    }
    let mut registered_structs = BTreeMap::new();
    for class in structs {
        let id_text: String = class.bind(py).getattr("__struct_id__")?.extract()?;
        let (id, expected_name) = expected_structs.remove(&id_text).ok_or_else(|| {
            py_value_error("registered struct has an unknown or duplicate __struct_id__")
        })?;
        let actual_name = class.bind(py).name()?;
        if actual_name.to_str()? != expected_name {
            return Err(py_value_error(
                "registered struct has the wrong generated class name",
            ));
        }
        if !pointers.insert(class.bind(py).as_ptr() as usize) {
            return Err(py_value_error("generated class registration is duplicated"));
        }
        registered_structs.insert(id, class);
    }
    let mut installed = InstalledRuntimeProjection::try_new(runtime).map_err(py_orm_error)?;
    if let Some(declared) = declared_schema_identity {
        installed = installed.with_declared_schema_identity(declared);
    }
    let installed = Arc::new(installed);
    let named_zone_marker = ordered_projection
        .then(|| named_zone_marker_class(py))
        .transpose()?;
    let named_zone_slots = match (successor_batch, named_zone_marker.as_ref()) {
        (true, Some(marker)) => Some(ProjectedNamedZoneSlots::capture(
            py,
            marker,
            Arc::clone(
                trusted_type_slots
                    .as_ref()
                    .expect("successor installation captured trusted type slots"),
            ),
        )?),
        _ => None,
    };
    let scalar_hydration = successor_batch
        .then(|| ProjectedScalarHydration::capture(py))
        .transpose()?;
    Ok(Arc::new(InstalledPackage {
        projection: installed,
        managed_scope_id,
        models: registered,
        structs: registered_structs,
        types_by_label,
        facade_origins: FacadeOriginRegistry::new(py)?,
        named_zone_marker,
        named_zone_slots,
        scalar_hydration,
    }))
}

fn trusted_python_heap_type_slots(py: Python<'_>) -> PyResult<Arc<ProjectedHeapAllocator>> {
    // Anchor directly to the immutable Stable-ABI type objects. Importing
    // `builtins` would allow pre-install monkeypatching to supply both the
    // alleged allocator and its probe family.
    let type_fn = unsafe {
        Bound::<PyAny>::from_borrowed_ptr(py, std::ptr::addr_of_mut!(pyo3::ffi::PyType_Type).cast())
    }
    .cast_into::<PyType>()?;
    let object = unsafe {
        Bound::<PyAny>::from_borrowed_ptr(
            py,
            std::ptr::addr_of_mut!(pyo3::ffi::PyBaseObject_Type).cast(),
        )
    }
    .cast_into::<PyType>()?;
    let object_new = py_getattr_cstr(py, object.as_any(), pyo3::ffi::c_str!("__new__"))?;
    let bases = PyTuple::new(py, [object.as_any()])?;
    let namespace = PyDict::new(py);
    let slots_name = fallible_python_string(py, "__slots__")?;
    namespace.set_item(slots_name.bind(py), PyTuple::empty(py))?;
    let probe_name = fallible_python_string(py, "_TypeBridgeGeneratedLayoutProbe")?;
    let probe = type_fn
        .call1((probe_name.bind(py), bases, namespace))?
        .cast_into::<PyType>()?;
    // SAFETY: the exact probe type is live under the GIL and both IDs are
    // Stable-ABI type slots.
    let deallocator = unsafe {
        pyo3::ffi::PyType_GetSlot(
            probe.as_ptr().cast::<pyo3::ffi::PyTypeObject>(),
            pyo3::ffi::Py_tp_dealloc,
        )
    } as usize;
    let constructor = type_slot(&object, pyo3::ffi::Py_tp_new) as usize;
    let trusted_free = type_slot(&probe, pyo3::ffi::Py_tp_free) as usize;
    let trusted_traverse = type_slot(&probe, pyo3::ffi::Py_tp_traverse) as usize;
    let trusted_clear = type_slot(&probe, pyo3::ffi::Py_tp_clear) as usize;
    let trusted_is_gc = type_slot(&probe, pyo3::ffi::Py_tp_is_gc) as usize;
    let trusted_gc_flags = unsafe { pyo3::ffi::PyType_GetFlags(probe.as_ptr().cast()) }
        & (pyo3::ffi::Py_TPFLAGS_HAVE_GC | pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END);
    if deallocator == 0 || constructor == 0 {
        return Err(py_runtime_error(
            "trusted Python heap layout probe omitted required type slots",
        ));
    }
    Ok(Arc::new(ProjectedHeapAllocator {
        object_new,
        trusted_deallocator: deallocator,
        trusted_constructor: constructor,
        trusted_free,
        trusted_traverse,
        trusted_clear,
        trusted_is_gc,
        trusted_gc_flags,
    }))
}

impl ProjectedScalarHydration {
    fn capture(py: Python<'_>) -> PyResult<Self> {
        let datetime = py.import("datetime")?;
        let date_type = exact_immutable_stdlib_type(&datetime, "date", "datetime")?;
        let datetime_type = exact_immutable_stdlib_type(&datetime, "datetime", "datetime")?;
        let timedelta_type = exact_immutable_stdlib_type(&datetime, "timedelta", "datetime")?;
        let timezone_type = exact_immutable_stdlib_type(&datetime, "timezone", "datetime")?;
        let zoneinfo = py.import("zoneinfo")?;
        let zoneinfo_type = exact_immutable_stdlib_type(&zoneinfo, "ZoneInfo", "zoneinfo")?;
        let decimal = py.import("decimal")?;
        let decimal_type = exact_immutable_stdlib_type(&decimal, "Decimal", "decimal")?;
        Ok(Self {
            datetime_replace: py_getattr_cstr(
                py,
                datetime_type.bind(py).as_any(),
                pyo3::ffi::c_str!("replace"),
            )?,
            tzinfo_key: fallible_python_string(py, "tzinfo")?,
            date_type,
            datetime_type,
            timedelta_type,
            timezone_type,
            zoneinfo_type,
            decimal_type,
        })
    }

    fn projected_to_py(
        &self,
        py: Python<'_>,
        value: &CanonicalValue,
        named_zone: Option<&ProjectedNamedZoneSlots>,
        pool: &BatchHydrationPool,
    ) -> PyResult<Py<PyAny>> {
        match value {
            CanonicalValue::String(value) => fallible_python_string(py, value.as_str()),
            CanonicalValue::Long(value) => fallible_python_i64(py, *value),
            CanonicalValue::Double(value) => {
                // SAFETY: Stable-ABI primitive constructor under the GIL.
                let value = unsafe { pyo3::ffi::PyFloat_FromDouble(value.get()) };
                // SAFETY: non-null is one owned exact-float reference.
                unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }.map(Bound::unbind)
            }
            CanonicalValue::Boolean(value) => {
                // SAFETY: Stable-ABI primitive constructor under the GIL.
                let value =
                    unsafe { pyo3::ffi::PyBool_FromLong(std::os::raw::c_long::from(*value)) };
                // SAFETY: non-null is one owned exact-bool reference.
                unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }.map(Bound::unbind)
            }
            CanonicalValue::Date(value) => self.date_from_canonical(py, *value),
            CanonicalValue::DateTime(value) => self.datetime_from_canonical(py, *value),
            CanonicalValue::DateTimeTz(value) => {
                self.datetime_tz_from_canonical(py, value, named_zone, pool)
            }
            CanonicalValue::Decimal(value) => {
                let value = fallible_python_string(py, value.as_str())?;
                self.decimal_type
                    .bind(py)
                    .call1((value.bind(py),))
                    .map(Bound::unbind)
            }
            CanonicalValue::Duration(value) => self.duration_from_canonical(py, *value),
        }
    }

    fn date_from_canonical(&self, py: Python<'_>, value: CanonicalDate) -> PyResult<Py<PyAny>> {
        let (year, month, day) = value.components();
        let year = fallible_python_i64(py, i64::from(year))?;
        let month = fallible_python_i64(py, i64::from(month))?;
        let day = fallible_python_i64(py, i64::from(day))?;
        let value = self
            .date_type
            .bind(py)
            .call1((year.bind(py), month.bind(py), day.bind(py)))?;
        if !value.get_type().is(self.date_type.bind(py)) {
            return Err(py_runtime_error("trusted date constructor changed type"));
        }
        Ok(value.unbind())
    }

    fn datetime_from_canonical(
        &self,
        py: Python<'_>,
        value: CanonicalDateTime,
    ) -> PyResult<Py<PyAny>> {
        let (year, month, day) = value.date().components();
        let (hour, minute, second, nanosecond) = value.time().components();
        if nanosecond % 1_000 != 0 {
            return Err(py_value_error(
                "datetime hydration requires microsecond precision",
            ));
        }
        let year = fallible_python_i64(py, i64::from(year))?;
        let month = fallible_python_i64(py, i64::from(month))?;
        let day = fallible_python_i64(py, i64::from(day))?;
        let hour = fallible_python_i64(py, i64::from(hour))?;
        let minute = fallible_python_i64(py, i64::from(minute))?;
        let second = fallible_python_i64(py, i64::from(second))?;
        let microsecond = fallible_python_i64(py, i64::from(nanosecond / 1_000))?;
        let value = self.datetime_type.bind(py).call1((
            year.bind(py),
            month.bind(py),
            day.bind(py),
            hour.bind(py),
            minute.bind(py),
            second.bind(py),
            microsecond.bind(py),
        ))?;
        if !value.get_type().is(self.datetime_type.bind(py)) {
            return Err(py_runtime_error(
                "trusted datetime constructor changed type",
            ));
        }
        Ok(value.unbind())
    }

    fn timedelta_from_components(
        &self,
        py: Python<'_>,
        days: i64,
        seconds: i64,
        micros: i64,
    ) -> PyResult<Py<PyAny>> {
        let days = fallible_python_i64(py, days)?;
        let seconds = fallible_python_i64(py, seconds)?;
        let micros = fallible_python_i64(py, micros)?;
        let value = self.timedelta_type.bind(py).call1((
            days.bind(py),
            seconds.bind(py),
            micros.bind(py),
        ))?;
        if !value.get_type().is(self.timedelta_type.bind(py)) {
            return Err(py_runtime_error(
                "trusted timedelta constructor changed type",
            ));
        }
        Ok(value.unbind())
    }

    fn duration_from_canonical(
        &self,
        py: Python<'_>,
        value: CanonicalDuration,
    ) -> PyResult<Py<PyAny>> {
        let (negative, months, days, seconds, nanosecond) = value.components();
        if negative || months != 0 || nanosecond % 1_000 != 0 {
            return Err(py_value_error(
                "duration hydration requires a nonnegative day-time value at microsecond precision",
            ));
        }
        let days = i64::try_from(days)
            .map_err(|_| py_value_error("duration hydration day count exceeds the Python range"))?;
        let seconds = i64::try_from(seconds).map_err(|_| {
            py_value_error("duration hydration second count exceeds the Python range")
        })?;
        self.timedelta_from_components(py, days, seconds, i64::from(nanosecond / 1_000))
    }

    fn datetime_tz_from_canonical(
        &self,
        py: Python<'_>,
        value: &CanonicalDateTimeTz,
        named_zone: Option<&ProjectedNamedZoneSlots>,
        pool: &BatchHydrationPool,
    ) -> PyResult<Py<PyAny>> {
        let resolved = self.datetime_from_canonical(py, value.local())?;
        let offset =
            self.timedelta_from_components(py, 0, i64::from(value.effective_offset_seconds()), 0)?;
        let offset = offset.into_bound(py);
        let timezone = match value.zone() {
            TimeZoneDesignator::Named(zone) => named_zone
                .ok_or_else(|| {
                    py_runtime_error("named-zone hydration requires installed successor slots")
                })?
                .allocate(py, offset.clone(), zone, pool)?,
            TimeZoneDesignator::Utc | TimeZoneDesignator::OffsetSeconds(_) => {
                let timezone = self.timezone_type.bind(py).call1((offset,))?;
                if !timezone.get_type().is(self.timezone_type.bind(py)) {
                    return Err(py_runtime_error(
                        "trusted timezone constructor changed type",
                    ));
                }
                timezone.unbind()
            }
        };
        let kwargs = fallible_python_dict(py)?;
        kwargs.set_item(self.tzinfo_key.bind(py), timezone)?;
        let replaced = self
            .datetime_replace
            .bind(py)
            .call((resolved.bind(py),), Some(&kwargs))?;
        if !replaced.get_type().is(self.datetime_type.bind(py)) {
            return Err(py_runtime_error(
                "trusted datetime replacement changed type",
            ));
        }
        Ok(replaced.unbind())
    }
}

impl ProjectedNamedZoneSlots {
    fn capture(
        py: Python<'_>,
        class: &Py<PyType>,
        heap_allocator: Arc<ProjectedHeapAllocator>,
    ) -> PyResult<Self> {
        let datetime = py.import("datetime")?;
        let base = exact_immutable_stdlib_type(&datetime, "tzinfo", "datetime")?;
        let constructor =
            py_getattr_cstr(py, base.bind(py).as_any(), pyo3::ffi::c_str!("__new__"))?;
        let trusted_constructor = type_slot(base.bind(py), pyo3::ffi::Py_tp_new) as usize;
        if trusted_constructor == 0 {
            return Err(py_runtime_error(
                "trusted datetime.tzinfo constructor slot is absent",
            ));
        }
        let allocator = ProjectedNamedZoneAllocator {
            constructor,
            trusted_constructor,
            base,
        };
        let trusted_mro = exact_type_mro(py, class.bind(py))?;
        validate_projected_named_zone_layout(
            py,
            class.bind(py),
            heap_allocator.as_ref(),
            &allocator,
            &trusted_mro,
        )?;
        let slots = Self {
            class: class.clone_ref(py),
            offset: ProjectedFacadeSlot::capture(py, class.bind(py), "_offset")?,
            zone: ProjectedFacadeSlot::capture(py, class.bind(py), "_type_bridge_zone")?,
            heap_allocator,
            allocator,
            trusted_mro,
        };
        slots.validate_layout(py)?;
        probe_projected_object_members(
            py,
            slots.class.bind(py),
            slots.allocator.constructor.bind(py),
            &[&slots.offset, &slots.zone],
        )?;
        Ok(slots)
    }

    fn validate_layout(&self, py: Python<'_>) -> PyResult<()> {
        let class = self.class.bind(py);
        validate_projected_named_zone_layout(
            py,
            class,
            self.heap_allocator.as_ref(),
            &self.allocator,
            &self.trusted_mro,
        )?;
        self.offset.validate_owner(py, class)?;
        self.zone.validate_owner(py, class)
    }

    fn allocate(
        &self,
        py: Python<'_>,
        offset: Bound<'_, PyAny>,
        zone: &str,
        pool: &BatchHydrationPool,
    ) -> PyResult<Py<PyAny>> {
        let zone = fallible_python_string(py, zone)?;
        // The all-model layout fence ran before provider polling and the GIL
        // has remained held without Python callbacks since then.
        let class = self.class.bind(py);
        let instance = self.allocator.constructor.bind(py).call1((class,))?;
        pool.retain_fresh(&instance, class)?;
        self.offset.set_infallible(py, &instance, &offset);
        self.zone.set_infallible(py, &instance, zone.bind(py));
        Ok(instance.unbind())
    }
}

fn validate_projected_named_zone_layout(
    py: Python<'_>,
    class: &Bound<'_, PyType>,
    heap: &ProjectedHeapAllocator,
    allocator: &ProjectedNamedZoneAllocator,
    trusted_mro: &[ProjectedTypeLayout],
) -> PyResult<()> {
    validate_exact_type_mro(py, class, trusted_mro)?;
    let object_pointer =
        std::ptr::addr_of_mut!(pyo3::ffi::PyBaseObject_Type).cast::<pyo3::ffi::PyObject>();
    if trusted_mro.len() != 3
        || !trusted_mro[0].class.bind(py).is(class)
        || !trusted_mro[1].class.bind(py).is(allocator.base.bind(py))
        || trusted_mro[2].class.bind(py).as_ptr() != object_pointer
    {
        return Err(py_type_error(
            "successor named-zone class changed its exact datetime.tzinfo MRO",
        ));
    }
    let flags = unsafe { pyo3::ffi::PyType_GetFlags(class.as_ptr().cast()) };
    if flags & pyo3::ffi::Py_TPFLAGS_HEAPTYPE == 0
        || flags & pyo3::ffi::Py_TPFLAGS_IS_ABSTRACT != 0
        || flags & pyo3::ffi::Py_TPFLAGS_HAVE_GC == 0
        || flags & pyo3::ffi::Py_TPFLAGS_IMMUTABLETYPE != 0
        || flags & pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END != 0
        || trusted_mro[0].itemsize != 0
        || trusted_mro[0].dictoffset != 0
    {
        return Err(py_type_error(
            "successor named-zone class must remain a concrete heap type",
        ));
    }
    let generic_allocator = pyo3::ffi::PyType_GenericAlloc as *const () as *mut std::ffi::c_void;
    if type_slot(class, pyo3::ffi::Py_tp_alloc) != generic_allocator
        || type_slot(class, pyo3::ffi::Py_tp_dealloc) as usize != heap.trusted_deallocator
        || type_slot(class, pyo3::ffi::Py_tp_new) as usize != allocator.trusted_constructor
        || type_slot(class, pyo3::ffi::Py_tp_free) as usize != heap.trusted_free
        || type_slot(class, pyo3::ffi::Py_tp_traverse) as usize != heap.trusted_traverse
        || type_slot(class, pyo3::ffi::Py_tp_clear) as usize != heap.trusted_clear
        || type_slot(class, pyo3::ffi::Py_tp_is_gc) as usize != heap.trusted_is_gc
        || flags & (pyo3::ffi::Py_TPFLAGS_HAVE_GC | pyo3::ffi::Py_TPFLAGS_ITEMS_AT_END)
            != heap.trusted_gc_flags
        || !type_slot(class, pyo3::ffi::Py_tp_finalize).is_null()
        || !type_slot(class, pyo3::ffi::Py_tp_del).is_null()
    {
        return Err(py_type_error(
            "successor named-zone class left its trusted datetime.tzinfo allocation family",
        ));
    }
    Ok(())
}

fn exact_immutable_stdlib_type(
    module: &Bound<'_, PyModule>,
    name: &'static str,
    module_name: &'static str,
) -> PyResult<Py<PyType>> {
    let py = module.py();
    let class = module.getattr(name)?.cast_into::<PyType>()?;
    // SAFETY: exact type pointer under the GIL; Stable-ABI flag query.
    let flags =
        unsafe { pyo3::ffi::PyType_GetFlags(class.as_ptr().cast::<pyo3::ffi::PyTypeObject>()) };
    if flags & pyo3::ffi::Py_TPFLAGS_IMMUTABLETYPE == 0 {
        return Err(py_type_error(format!(
            "successor scalar type {module_name}.{name} is not immutable",
        )));
    }
    let actual_name: String = py_getattr_cstr(py, class.as_any(), pyo3::ffi::c_str!("__name__"))?
        .bind(py)
        .extract()?;
    let actual_module: String =
        py_getattr_cstr(py, class.as_any(), pyo3::ffi::c_str!("__module__"))?
            .bind(py)
            .extract()?;
    if actual_name != name || actual_module != module_name {
        return Err(py_type_error(format!(
            "successor scalar type {module_name}.{name} has untrusted identity",
        )));
    }
    Ok(class.unbind())
}

fn fallible_python_string(py: Python<'_>, value: &str) -> PyResult<Py<PyAny>> {
    let length = pyo3::ffi::Py_ssize_t::try_from(value.len())
        .map_err(|_| py_value_error("Python string exceeds Py_ssize_t"))?;
    // SAFETY: UTF-8 bytes remain live for the call; explicit length permits
    // embedded NUL and the Stable-ABI constructor copies them.
    let value = unsafe { pyo3::ffi::PyUnicode_FromStringAndSize(value.as_ptr().cast(), length) };
    // SAFETY: non-null is one owned exact-str reference.
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }.map(Bound::unbind)
}

fn fallible_python_i64(py: Python<'_>, value: i64) -> PyResult<Py<PyAny>> {
    // SAFETY: Stable-ABI exact-int constructor under the GIL.
    let value = unsafe { pyo3::ffi::PyLong_FromLongLong(value) };
    // SAFETY: non-null is one owned exact-int reference.
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }.map(Bound::unbind)
}

fn fallible_python_dict(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    // SAFETY: Stable-ABI exact-dict constructor under the GIL.
    let value = unsafe { pyo3::ffi::PyDict_New() };
    // SAFETY: non-null is one owned exact-dict reference.
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }?
        .cast_into_exact::<PyDict>()
        .map_err(Into::into)
}

fn fallible_python_empty_list(py: Python<'_>) -> PyResult<Py<PyAny>> {
    // SAFETY: Stable-ABI exact-list constructor under the GIL.
    let value = unsafe { pyo3::ffi::PyList_New(0) };
    // SAFETY: non-null is one owned exact-list reference.
    unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, value) }.map(Bound::unbind)
}

fn named_zone_marker_class(py: Python<'_>) -> PyResult<Py<PyType>> {
    PyModule::from_code(
        py,
        pyo3::ffi::c_str!(
            r#"
import datetime as _datetime

class _TypeBridgeNamedZone(_datetime.tzinfo):
    __slots__ = ("_offset", "_type_bridge_zone")

    def __init__(self, offset_seconds, zone):
        self._offset = _datetime.timedelta(seconds=offset_seconds)
        self._type_bridge_zone = zone

    def utcoffset(self, _datetime_value):
        return self._offset

    def dst(self, _datetime_value):
        return _datetime.timedelta(0)

    def tzname(self, _datetime_value):
        return self._type_bridge_zone

    def fromutc(self, datetime_value):
        if datetime_value.tzinfo is not self:
            raise ValueError("fromutc requires this exact timezone marker")
        return datetime_value + self._offset
"#
        ),
        pyo3::ffi::c_str!("_type_bridge_named_zone.py"),
        pyo3::ffi::c_str!("_type_bridge_native"),
    )?
    .getattr("_TypeBridgeNamedZone")?
    .cast_into::<PyType>()
    .map(Bound::unbind)
    .map_err(|error| py_runtime_error(error.to_string()))
}

fn install_authority_backed_projection(
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    schema_authority: &[u8],
) -> PyResult<(
    RuntimeProjection,
    ManagedScopeId,
    DeclaredIdentityFingerprint,
)> {
    let rejection = || projection_evidence_rejection(semantic_fingerprint_json);
    let runtime = decode_runtime_projection_verified(
        projection_json.as_bytes(),
        semantic_fingerprint_json.as_bytes(),
        projection_fingerprint_json.as_bytes(),
    )
    .map_err(|_| rejection())?;
    if runtime.target() != BindingTarget::Python {
        return Err(rejection());
    }
    let authority =
        decode_schema_authority(schema_authority, &schema_authority_capability_vocabulary())
            .map_err(|_| rejection())?;
    verify_projection_evidence(&authority, &runtime).map_err(|_| rejection())?;
    let managed_scope_id = authority.managed_scope().id().clone();
    let declared_schema_identity = authority
        .resolved_schema()
        .declared_identity_fingerprint()
        .clone();
    Ok((runtime, managed_scope_id, declared_schema_identity))
}

fn projection_evidence_mismatch() -> PyErr {
    py_sdk_diagnostic(SdkExecutionDiagnostic::projection_evidence_mismatch())
}

fn projection_evidence_rejection(semantic_fingerprint_json: &str) -> PyErr {
    let presence = if semantic_fingerprint_json.is_empty() {
        SdkProjectionEvidenceSlotPresence::Absent
    } else {
        SdkProjectionEvidenceSlotPresence::Present
    };
    py_sdk_diagnostic(
        SdkExecutionDiagnostic::classify_detached_semantic_schema_fingerprint_rejection(presence),
    )
}

fn verify_legacy_python_projection_evidence(runtime: &RuntimeProjection) -> PyResult<()> {
    if projection_uses_ordered_collections(runtime) {
        return Err(py_value_error(
            "ordered Python runtime projections require compiled schema authority",
        ));
    }
    let emitter = PythonEmitter::new();
    let resources = emitter.code_resources().map_err(py_diagnostic)?;
    if runtime.config() != &ProjectionConfig::python()
        || runtime.generator_handlers() != emitter.generator_handlers()
        || runtime.code_resources() != resources
    {
        return Err(py_value_error(
            "legacy Python runtime projection does not match the exact shipped handler and resource evidence",
        ));
    }
    Ok(())
}

fn projection_uses_ordered_collections(projection: &RuntimeProjection) -> bool {
    projection.models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
}

fn verify_class(
    py: Python<'_>,
    class: &Py<PyType>,
    expected_name: &str,
    expected_form: &str,
    pointers: &mut BTreeSet<usize>,
) -> PyResult<()> {
    let class = class.bind(py);
    let name: String = class.getattr("__name__")?.extract()?;
    let form: String = class.getattr("__model_form__")?.extract()?;
    if name != expected_name || form != expected_form {
        return Err(py_value_error(format!(
            "registered class {name:?} does not match projected {expected_form} class {expected_name:?}"
        )));
    }
    if !pointers.insert(class.as_ptr() as usize) {
        return Err(py_value_error(
            "one Python class was registered for multiple projected forms",
        ));
    }
    Ok(())
}

fn canonical_id(id: &TypeId) -> PyResult<String> {
    String::from_utf8(to_canonical_json(id).map_err(py_diagnostic)?)
        .map_err(|error| py_runtime_error(error.to_string()))
}

fn ensure_manageable(package: &InstalledPackage, id: &TypeId) -> PyResult<()> {
    if package.projection.descriptor(id).is_err() {
        return Err(py_type_error(
            "attribute projections do not expose CRUD managers",
        ));
    }
    Ok(())
}

fn compatibility_clause_body(clause: Clause, keyword: &str) -> PyResult<String> {
    let compiled = QueryCompiler::new().compile_clause(&clause);
    let prefix = format!("{keyword}\n");
    compiled
        .strip_prefix(&prefix)
        .and_then(|body| body.strip_suffix(';'))
        .map(str::to_owned)
        .ok_or_else(|| py_runtime_error("native query compiler returned an invalid clause shape"))
}

fn lower_attributes(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    instance: &Bound<'_, PyAny>,
) -> PyResult<DynamicAttributeMap> {
    let values = instance.call_method0("runtime_values")?;
    let values = values.cast::<PyDict>()?;
    let mut attributes = Vec::new();
    for descriptor in descriptors {
        let value = values.get_item(&descriptor.field_name)?;
        let items = normalized_items(value.as_ref(), descriptor_cardinality(descriptor))?;
        for item in items {
            let (id, form) = package.identify_value(py, &item)?;
            let expected = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
            if &id != expected || form != ProjectedModelForm::Complete {
                return Err(py_type_error(format!(
                    "field {:?} requires its exact complete attribute wrapper",
                    descriptor.field_name
                )));
            }
            let scalar = item.call_method0("runtime_attribute_value")?;
            attributes.push((
                descriptor.attr_name.clone(),
                attribute_value_from_py(py, &scalar, descriptor.value_type)?,
            ));
        }
    }
    Ok(attributes)
}

fn project_create(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    instance: &Bound<'_, PyAny>,
) -> PyResult<ProjectedCreate> {
    project_create_at(py, package, id, instance, &[])
}

fn hydrate_projected_create(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: &ProjectedCreate,
) -> PyResult<Py<PyAny>> {
    projected
        .validate_for(&package.projection)
        .map_err(py_sdk_diagnostic)?;
    let id = projected.type_id();
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projected create model is absent"))?;
    let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
        TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
    };
    let values = PyDict::new(py);
    for field in model.create().fields() {
        if !projected.field_is_present(field.token()) {
            continue;
        }
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected create field has no query token"))?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == token.target_name().as_str())
            .ok_or_else(|| py_runtime_error("projected create field has no descriptor"))?;
        let projected_values = projected
            .fields()
            .get(field.token())
            .map_or_else(|| &[][..], Vec::as_slice);
        let mut hydrated = Vec::with_capacity(projected_values.len());
        for value in projected_values {
            hydrated.push(hydrate_attribute(
                py,
                package,
                descriptor,
                &value.to_attribute_value(),
            )?);
        }
        set_projected_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            hydrated,
            field.multiplicity(),
        )?;
    }
    for (role_id, role) in model.create().roles() {
        if !projected.role_is_present(role_id) {
            continue;
        }
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| py_runtime_error("projected create role has no query token"))?;
        let references = projected
            .roles()
            .get(role_id)
            .map_or_else(|| &[][..], Vec::as_slice);
        let mut hydrated = Vec::with_capacity(references.len());
        for reference in references {
            let reference_values = hydrate_projected_fields(
                py,
                package,
                reference.type_id(),
                HydratedProjectedFields::Reference(reference.keys()),
                false,
                None,
            )?;
            hydrated.push(allocate_projected_detached_reference(
                py,
                package,
                reference.type_id(),
                &reference_values,
                reference.iid(),
            )?);
        }
        set_projected_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            hydrated,
            role.multiplicity(),
        )?;
    }
    let instance = allocate(py, package.class(id, ProjectedModelForm::Complete)?)?;
    instance.call_method1("initialize_runtime_values", (&values,))?;
    Ok(instance.unbind())
}

fn project_create_at(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    instance: &Bound<'_, PyAny>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedCreate> {
    let values = py_getattr_cstr(py, instance, pyo3::ffi::c_str!("runtime_values"))?;
    let values = values.bind(py).call0()?;
    let values = values.cast::<PyDict>()?;
    project_create_from_values_at(py, package, id, values, operation_path, None)
}

fn project_create_from_snapshot_at(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    snapshot: &ProjectedFacadeSnapshot,
    operation_path: &[SdkDiagnosticPathSegment],
    batch_budget: &ProjectedBatchBindingBudget<'_>,
) -> PyResult<ProjectedCreate> {
    let values_path =
        extended_projected_path(operation_path, [SdkDiagnosticPathSegment::Type(id.clone())])?;
    let values = snapshot
        .values
        .bind(py)
        .cast_exact::<PyDict>()
        .map_err(|_| {
            py_sdk_diagnostic(batch_input_shape_diagnostic(
                "generated_model_values_type_mismatch",
                "The generated model runtime values slot must contain an exact dictionary",
                &values_path,
            ))
        })?;
    project_create_from_values_at(py, package, id, values, operation_path, Some(batch_budget))
}

fn project_create_from_values_at(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    operation_path: &[SdkDiagnosticPathSegment],
    batch_budget: Option<&ProjectedBatchBindingBudget<'_>>,
) -> PyResult<ProjectedCreate> {
    let projection = package.projection.projection();
    let model = projection
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projection model is absent"))?;
    let mut create_budget =
        ProjectedCreateBudget::try_new(&package.projection, id).map_err(|diagnostic| {
            py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
        })?;

    let mut fields = Vec::new();
    fields
        .try_reserve_exact(model.create().fields().len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    for field in model.create().fields() {
        if let Some(batch_budget) = batch_budget {
            batch_budget.checkpoint().map_err(py_sdk_diagnostic)?;
        }
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected create field has no query token"))?;
        let target_name = fallible_python_string(py, token.target_name().as_str())?;
        let value = values.get_item(target_name.bind(py))?;
        if value.is_none() || value.as_ref().is_some_and(Bound::is_none) {
            continue;
        }
        let collection_path = extended_projected_path(
            operation_path,
            [
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(field.token().clone()),
            ],
        )?;
        let mut projected = Vec::new();
        visit_projected_items(
            value.as_ref(),
            field.multiplicity(),
            &collection_path,
            |index, value| {
                if let Some(batch_budget) = batch_budget {
                    batch_budget.checkpoint().map_err(py_sdk_diagnostic)?;
                }
                let path = extended_projected_path(
                    &collection_path,
                    [SdkDiagnosticPathSegment::Index(projected_index(index))],
                )?;
                let value = project_attribute_value(py, package, &value, &path)?;
                create_budget
                    .try_add_field_value(&package.projection, field.token(), index, &value)
                    .map_err(|diagnostic| {
                        py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
                    })?;
                projected
                    .try_reserve(1)
                    .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
                projected.push(value);
                Ok(())
            },
        )?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::new();
    roles
        .try_reserve_exact(model.create().roles().len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    for (role_id, role) in model.create().roles() {
        if let Some(batch_budget) = batch_budget {
            batch_budget.checkpoint().map_err(py_sdk_diagnostic)?;
        }
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| py_runtime_error("projected create role has no query token"))?;
        let target_name = fallible_python_string(py, token.target_name().as_str())?;
        let value = values.get_item(target_name.bind(py))?;
        if value.is_none() || value.as_ref().is_some_and(Bound::is_none) {
            continue;
        }
        let collection_path = extended_projected_path(
            operation_path,
            [
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
            ],
        )?;
        let mut projected = Vec::new();
        visit_projected_items(
            value.as_ref(),
            role.multiplicity(),
            &collection_path,
            |index, value| {
                if let Some(batch_budget) = batch_budget {
                    batch_budget.checkpoint().map_err(py_sdk_diagnostic)?;
                }
                let path = extended_projected_path(
                    &collection_path,
                    [SdkDiagnosticPathSegment::Index(projected_index(index))],
                )?;
                let reference = project_reference(py, package, &value, role.players(), &path)?;
                create_budget
                    .try_add_role_reference(&package.projection, role_id, index, &reference)
                    .map_err(|diagnostic| {
                        py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
                    })?;
                projected
                    .try_reserve(1)
                    .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
                projected.push(reference);
                Ok(())
            },
        )?;
        roles.push((role_id.clone(), projected));
    }

    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles).map_err(|diagnostic| {
        py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
    })
}

fn extended_projected_path<const N: usize>(
    prefix: &[SdkDiagnosticPathSegment],
    suffix: [SdkDiagnosticPathSegment; N],
) -> PyResult<Vec<SdkDiagnosticPathSegment>> {
    let capacity = prefix
        .len()
        .checked_add(N)
        .ok_or_else(|| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    let mut path = Vec::new();
    path.try_reserve_exact(capacity)
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    path.extend(prefix.iter().cloned());
    path.extend(suffix);
    Ok(path)
}

fn prefix_projected_diagnostic(
    diagnostic: SdkExecutionDiagnostic,
    prefix: &[SdkDiagnosticPathSegment],
) -> SdkExecutionDiagnostic {
    diagnostic
        .try_with_path_prefix(prefix.iter().cloned())
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
}

fn project_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedAttributeValue> {
    let (attribute_id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if attribute_id.kind() != TypeKind::Attribute || form != ProjectedModelForm::Complete {
        return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
            "projected_field_value_form_mismatch",
            "Projected field values must use an exact complete attribute wrapper",
            operation_path,
        )));
    }
    let attribute = package
        .projection
        .projection()
        .models()
        .get(&attribute_id)
        .ok_or_else(|| py_runtime_error("projection field attribute is absent"))?;
    let value_type = attribute
        .declaration()
        .value_type()
        .ok_or_else(|| py_runtime_error("projection field attribute has no scalar domain"))?;
    let slots = package.batch_slots_for(&attribute_id, ProjectedModelForm::Complete)?;
    let scalar = slots
        .attribute_value
        .as_ref()
        .ok_or_else(|| py_runtime_error("successor attribute storage slot was not installed"))?
        .get(py, value)
        .map_err(|_| {
            py_sdk_diagnostic(batch_input_shape_diagnostic(
                "projected_attribute_value_slot_uninitialized",
                "Projected field attribute storage is not initialized",
                operation_path,
            ))
        })?;
    let scalar = scalar.bind(py);
    let scalar_hydration = package
        .scalar_hydration
        .as_ref()
        .ok_or_else(|| py_runtime_error("successor scalar hydration plan was not installed"))?;
    let scalar = canonical_projected_value_from_py_at(
        py,
        scalar,
        projected_value_type(value_type),
        scalar_hydration,
        package.named_zone_marker.as_ref(),
        package.named_zone_slots.as_ref(),
        operation_path,
    )?;
    ProjectedAttributeValue::try_new(&package.projection, attribute_id, scalar).map_err(
        |diagnostic| py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path)),
    )
}

fn project_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    allowed_players: &BTreeSet<ProjectedModelUse>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedReference> {
    let (id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
            "projected_role_player_kind_mismatch",
            "Projected role players must use an exact entity or relation model",
            operation_path,
        )));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(py_sdk_diagnostic(batch_input_shape_diagnostic(
            "projected_role_player_form_mismatch",
            "Projected role player uses a generated model form outside this exact role",
            operation_path,
        )));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| py_runtime_error("projection role-player model is absent"))?;
    let slots = package.batch_slots_for(&id, form)?;
    let values_path =
        extended_projected_path(operation_path, [SdkDiagnosticPathSegment::Type(id.clone())])?;
    let values = slots.values.get(py, value).map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "projected_role_player_values_slot_uninitialized",
            "Projected role-player runtime values storage is not initialized",
            &values_path,
        ))
    })?;
    let values = values.bind(py);
    let values = values.cast_exact::<PyDict>().map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "projected_role_player_values_type_mismatch",
            "Projected role-player runtime values must be an exact dictionary",
            &values_path,
        ))
    })?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| py_runtime_error("projected reference key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| py_runtime_error("projected reference key has no read field"))?;
        let target_name = fallible_python_string(py, token.target_name().as_str())?;
        let key = values.get_item(target_name.bind(py))?;
        let key_collection_path = extended_projected_path(
            operation_path,
            [
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(key_id.clone()),
            ],
        )?;
        visit_projected_items(
            key.as_ref(),
            read.multiplicity(),
            &key_collection_path,
            |index, item| {
                let key_path = extended_projected_path(
                    &key_collection_path,
                    [SdkDiagnosticPathSegment::Index(projected_index(index))],
                )?;
                let projected = project_attribute_value(py, package, &item, &key_path)?;
                keys.try_reserve(1)
                    .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
                keys.push((key_id.clone(), projected));
                Ok(())
            },
        )?;
    }
    let iid = projected_reference_iid(py, value, &id, slots.as_ref(), operation_path)?;
    let proof = package.facade_origins.proof(value);
    let Some(proof) = proof else {
        return ProjectedReference::try_new(&package.projection, id, iid, keys).map_err(
            |diagnostic| py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path)),
        );
    };
    let origin = proof.origin_carrier().map_err(|diagnostic| {
        py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
    })?;
    let visible =
        ProjectedReference::try_new_with_origin_carrier(&package.projection, id, iid, keys, origin)
            .map_err(|diagnostic| {
                py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
            })?;
    let expected = proof.reference(&package.projection).map_err(|diagnostic| {
        py_sdk_diagnostic(prefix_projected_diagnostic(diagnostic, operation_path))
    })?;
    if visible != expected {
        return Err(py_sdk_diagnostic(facade_projection_evidence_mismatch(
            operation_path,
        )));
    }
    Ok(visible)
}

fn projected_reference_iid(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    id: &TypeId,
    slots: &ProjectedFacadeSlots,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<Option<String>> {
    let iid_path = extended_projected_path(
        operation_path,
        [
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Argument(
                SdkDiagnosticName::new("iid").expect("the fixed IID argument is canonical"),
            ),
        ],
    )?;
    let iid = slots.iid.get(py, value).map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "reference_iid_slot_uninitialized",
            "Projected role-player IID storage is not initialized",
            &iid_path,
        ))
    })?;
    let iid = iid.bind(py);
    if iid.is_none() {
        return Ok(None);
    }
    let iid = iid.cast_exact::<PyString>().map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "reference_iid_type_mismatch",
            "Projected role-player IIDs must be exact strings",
            &iid_path,
        ))
    })?;
    let iid = iid.to_str().map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "reference_iid_type_mismatch",
            "Projected role-player IIDs must be UTF-8 strings",
            &iid_path,
        ))
    })?;
    copy_canonical_iid_at(
        iid,
        &iid_path,
        "The reference IID is not canonical TypeDB identity text",
    )
    .map(Some)
}

fn project_hydrated_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: Option<&str>,
) -> PyResult<ProjectedThing> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projection hydrated model is absent"))?;
    let mut fields = Vec::with_capacity(model.complete_read().fields().len());
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected read field has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        if field.multiplicity().container() == ProjectedContainer::Scalar
            && value.as_ref().is_none_or(Bound::is_none)
        {
            continue;
        }
        let projected = projected_items(value.as_ref(), field.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                project_hydrated_attribute_value(
                    py,
                    package,
                    &value,
                    &[
                        SdkDiagnosticPathSegment::Type(id.clone()),
                        SdkDiagnosticPathSegment::Field(field.token().clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::with_capacity(model.complete_read().roles().len());
    for (role_id, role) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| py_runtime_error("projected read role has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        let players = projected_items(value.as_ref(), role.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let path = [
                    SdkDiagnosticPathSegment::Type(id.clone()),
                    SdkDiagnosticPathSegment::Role(role_id.clone()),
                    SdkDiagnosticPathSegment::Index(projected_index(index)),
                ];
                project_hydrated_role_player(py, package, &value, role, &path)
            })
            .collect::<PyResult<Vec<_>>>()?;
        roles.push((role_id.clone(), players));
    }

    ProjectedThing::try_new(
        &package.projection,
        id.clone(),
        iid.unwrap_or_default().to_owned(),
        fields,
        roles,
    )
    .map_err(py_sdk_diagnostic)
}

fn project_hydrated_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedAttributeValue> {
    let (attribute_id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if attribute_id.kind() != TypeKind::Attribute || form != ProjectedModelForm::Complete {
        return Err(py_type_error(
            "hydrated field value is not an exact complete attribute wrapper",
        ));
    }
    let attribute = package
        .projection
        .projection()
        .models()
        .get(&attribute_id)
        .ok_or_else(|| py_runtime_error("projection hydrated attribute is absent"))?;
    let value_type = attribute
        .declaration()
        .value_type()
        .ok_or_else(|| py_runtime_error("projection hydrated attribute has no scalar domain"))?;
    let scalar = value.call_method0("runtime_attribute_value")?;
    let scalar = canonical_attribute_value_from_py(
        py,
        &scalar,
        projected_value_type(value_type),
        package.named_zone_marker.as_ref(),
    )?;
    ProjectedAttributeValue::try_from_hydrated_attribute_value(
        &package.projection,
        attribute_id,
        &scalar,
    )
    .map_err(py_sdk_diagnostic)
}

fn project_hydrated_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    allowed_players: &BTreeSet<ProjectedModelUse>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedReference> {
    let (id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_type_error(
            "hydrated role player is not an exact entity or relation value",
        ));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(py_type_error(
            "hydrated role player has an incompatible generated model form",
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| py_runtime_error("projection hydrated role-player model is absent"))?;
    let values = value.call_method0("runtime_values")?;
    let values = values.cast::<PyDict>()?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| py_runtime_error("projected hydrated key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| py_runtime_error("projected hydrated key has no read field"))?;
        let key = values.get_item(token.target_name().as_str())?;
        for (index, item) in projected_items(key.as_ref(), read.multiplicity())?
            .into_iter()
            .enumerate()
        {
            let key_path = extended_projected_path(
                operation_path,
                [
                    SdkDiagnosticPathSegment::Type(id.clone()),
                    SdkDiagnosticPathSegment::Field(key_id.clone()),
                    SdkDiagnosticPathSegment::Index(projected_index(index)),
                ],
            )?;
            keys.push((
                key_id.clone(),
                project_hydrated_attribute_value(py, package, &item, &key_path)?,
            ));
        }
    }
    ProjectedReference::try_new_for_hydration(&package.projection, id, projected_iid(value)?, keys)
        .map_err(py_sdk_diagnostic)
}

fn project_hydrated_role_player(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    read_role: &type_bridge_contract::projection::ReadRoleProjection,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedRolePlayer> {
    let (id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    let reference =
        project_hydrated_reference(py, package, value, read_role.players(), operation_path)?;
    if form == ProjectedModelForm::Reference {
        return ProjectedRolePlayer::try_new_reference_for_hydration(
            &package.projection,
            read_role,
            reference,
        )
        .map_err(py_sdk_diagnostic);
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| py_runtime_error("projection hydrated role-player model is absent"))?;
    let values = value.call_method0("runtime_values")?;
    let values = values.cast_exact::<PyDict>()?;
    let mut fields = Vec::with_capacity(model.complete_read().fields().len());
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected role-player field has no query token"))?;
        let raw = values.get_item(token.target_name().as_str())?;
        if field.multiplicity().container() == ProjectedContainer::Scalar
            && raw.as_ref().is_none_or(Bound::is_none)
        {
            continue;
        }
        let projected = projected_items(raw.as_ref(), field.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let path = extended_projected_path(
                    operation_path,
                    [
                        SdkDiagnosticPathSegment::Type(id.clone()),
                        SdkDiagnosticPathSegment::Field(field.token().clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )?;
                project_hydrated_attribute_value(py, package, &item, &path)
            })
            .collect::<PyResult<Vec<_>>>()?;
        fields.push((field.token().clone(), projected));
    }
    ProjectedRolePlayer::try_new_complete_for_hydration(
        &package.projection,
        read_role,
        reference,
        fields,
    )
    .map_err(py_sdk_diagnostic)
}

fn projected_items<'py>(
    value: Option<&Bound<'py, PyAny>>,
    multiplicity: ProjectedMultiplicity,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    match value {
        None => Ok(Vec::new()),
        Some(value) if value.is_none() => Ok(Vec::new()),
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            Ok(vec![value.clone()])
        }
        Some(value) => value
            .cast::<PyTuple>()
            .map_err(|_| py_type_error("projected sequence input is not normalized as a tuple"))
            .map(|values| values.iter().collect()),
    }
}

fn visit_projected_items<'py>(
    value: Option<&Bound<'py, PyAny>>,
    multiplicity: ProjectedMultiplicity,
    path: &[SdkDiagnosticPathSegment],
    mut visitor: impl FnMut(usize, Bound<'py, PyAny>) -> PyResult<()>,
) -> PyResult<()> {
    match value {
        None => Ok(()),
        Some(value) if value.is_none() => Ok(()),
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            visitor(0, value.clone())
        }
        Some(value) => {
            let values = value.cast_exact::<PyTuple>().map_err(|_| {
                py_sdk_diagnostic(batch_input_shape_diagnostic(
                    "projected_collection_container_mismatch",
                    "Projected sequence input must use the generated exact tuple container",
                    path,
                ))
            })?;
            for (index, value) in values.iter().enumerate() {
                visitor(index, value)?;
            }
            Ok(())
        }
    }
}

fn generated_token_package_mismatch_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    path.into_iter().fold(
        SdkExecutionDiagnostic::generated_token_package_mismatch(),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .expect("a generated create operation path fits the SDK diagnostic contract")
        },
    )
}

fn facade_projection_evidence_mismatch(
    path: &[SdkDiagnosticPathSegment],
) -> SdkExecutionDiagnostic {
    path.iter().cloned().fold(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("hydrated_facade_evidence_mismatch")
                .expect("static hydrated-facade code is canonical"),
            SdkDiagnosticMessage::new(
                "The hydrated facade no longer matches its retained projection evidence",
            )
            .expect("static hydrated-facade message is canonical"),
        ),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .expect("a generated role-player path fits the SDK diagnostic contract")
        },
    )
}

fn projected_index(index: usize) -> u64 {
    u64::try_from(index).expect("a projected collection index fits the SDK diagnostic contract")
}

fn initial_compatibility_filter(
    package: &InstalledPackage,
    model: &TypeId,
) -> PyResult<Option<ProjectedManagerFilter>> {
    if package.projection.projection().generator_handlers()
        != [type_bridge_contract::projection::ProjectionHandler::python_v2()]
    {
        return Ok(None);
    }
    ProjectedManagerFilter::try_new(&package.projection, model.clone())
        .map(Some)
        .map_err(py_sdk_diagnostic)
}

/// Preserve the released keyword-filter grammar while routing only its closed,
/// exact-wrapper comparison subset through the canonical manager executor.
/// Iterating the Python dict directly retains authored predicate order.
fn lower_compatibility_filter_kwargs(
    py: Python<'_>,
    package: &InstalledPackage,
    model: &TypeId,
    descriptors: &[OwnedAttributeDescriptor],
    filters: Option<&Bound<'_, PyDict>>,
    base: &ProjectedManagerFilter,
) -> PyResult<Option<ProjectedManagerFilter>> {
    let Some(filters) = filters else {
        return Ok(Some(base.clone()));
    };
    let mut lowered = base.clone();
    for (key, value) in filters {
        let key = key
            .cast::<PyString>()
            .map_err(|_| py_type_error("generated manager filter names must be strings"))?
            .to_str()?;
        if matches!(
            key,
            "iid" | "_iid" | "iid__eq" | "_iid__eq" | "iid__in" | "_iid__in"
        ) {
            return Ok(None);
        }
        let has_field = |name: &str| {
            descriptors
                .iter()
                .any(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
        };
        let resolved = resolve_generated_manager_lookup(key, has_field);
        let (field_name, lookup) = (resolved.field_name(), resolved.lookup());
        let comparison = match lookup {
            "eq" | "exact" => ProjectedManagerComparison::Eq,
            "ne" => ProjectedManagerComparison::Ne,
            "gt" => ProjectedManagerComparison::Gt,
            "gte" => ProjectedManagerComparison::Gte,
            "lt" => ProjectedManagerComparison::Lt,
            "lte" => ProjectedManagerComparison::Lte,
            _ => return Ok(None),
        };
        let Some(descriptor) = descriptors.iter().find(|descriptor| {
            descriptor.field_name == field_name || descriptor.attr_name == field_name
        }) else {
            return Ok(None);
        };
        if descriptor.value_type == ValueType::Boolean
            && !matches!(
                comparison,
                ProjectedManagerComparison::Eq | ProjectedManagerComparison::Ne
            )
        {
            return Ok(None);
        }
        let expected = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
        if !matches!(
            package.identify_value(py, &value),
            Ok((ref attribute, ProjectedModelForm::Complete)) if attribute == expected
        ) {
            return Ok(None);
        }
        let field = package
            .projection
            .projection()
            .models()
            .get(model)
            .and_then(|projection| {
                projection
                    .query_tokens()
                    .fields()
                    .values()
                    .find(|field| field.target_name().as_str() == field_name)
            })
            .ok_or_else(|| py_runtime_error("projected manager field token is absent"))?;
        let projected = project_attribute_value(py, package, &value, &[])?;
        lowered = lowered
            .try_and(
                &package.projection,
                &ProjectedTokenIdentity::Field {
                    owner: model.clone(),
                    field: field.id().clone(),
                },
                comparison,
                &projected,
            )
            .map_err(py_sdk_diagnostic)?;
    }
    Ok(Some(lowered))
}

fn lower_filter_kwargs(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    filters: Option<&Bound<'_, PyDict>>,
) -> PyResult<Vec<DynamicExpr>> {
    let Some(filters) = filters else {
        return Ok(vec![]);
    };
    let mut lowered = Vec::with_capacity(filters.len());
    for (key, value) in filters {
        let key = key
            .cast::<PyString>()
            .map_err(|_| py_type_error("generated manager filter names must be strings"))?
            .to_str()?;
        if matches!(key, "iid" | "_iid" | "iid__eq" | "_iid__eq") {
            lowered.push(DynamicExpr::Iid {
                iid: projected_filter_iid(&value)?,
            });
            continue;
        }
        if matches!(key, "iid__in" | "_iid__in") {
            let iids = projected_filter_items(&value, "iid__in")?
                .iter()
                .map(projected_filter_iid)
                .collect::<PyResult<Vec<_>>>()?;
            lowered.push(DynamicExpr::Or {
                exprs: iids
                    .into_iter()
                    .map(|iid| DynamicExpr::Iid { iid })
                    .collect(),
            });
            continue;
        }
        // A generated field may itself contain `__`. A recognised trailing
        // lookup wins only when the prefix is also a field, so `score__gte`
        // remains the comparison on `score`. Use `score__gte__eq` to select
        // equality on a field literally named `score__gte`.
        let parsed_lookup = key.rsplit_once("__");
        let has_field = |name: &str| {
            descriptors
                .iter()
                .any(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
        };
        let (field_name, lookup) = match parsed_lookup {
            Some((field_name, lookup))
                if matches!(
                    lookup,
                    "eq" | "exact"
                        | "ne"
                        | "gt"
                        | "gte"
                        | "lt"
                        | "lte"
                        | "contains"
                        | "startswith"
                        | "endswith"
                        | "regex"
                        | "like"
                        | "in"
                        | "isnull"
                ) && has_field(field_name) =>
            {
                (field_name, lookup)
            }
            _ if has_field(key) => (key, "eq"),
            Some((field_name, lookup)) => (field_name, lookup),
            None => (key, "eq"),
        };
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == field_name || descriptor.attr_name == field_name
            })
            .ok_or_else(|| {
                py_value_error(format!("unknown generated manager filter {field_name:?}"))
            })?;
        if matches!(
            lookup,
            "contains" | "startswith" | "endswith" | "regex" | "like"
        ) && descriptor.value_type != ValueType::String
        {
            return Err(py_value_error(format!(
                "unsupported generated manager lookup {lookup:?} for non-string field {field_name:?}"
            )));
        }
        if lookup == "isnull" {
            let is_null = value
                .cast_exact::<PyBool>()
                .map_err(|_| py_type_error("generated manager isnull lookup requires a bool"))?
                .extract::<bool>()?;
            lowered.push(DynamicExpr::IsNull {
                attr_name: descriptor.attr_name.clone(),
                is_null,
            });
            continue;
        }
        if lookup == "in" {
            let exprs = projected_filter_items(&value, "in")?
                .iter()
                .map(|item| {
                    Ok(DynamicExpr::Compare {
                        attr_name: descriptor.attr_name.clone(),
                        operator: DynamicComparisonOp::Eq,
                        value: projected_filter_attribute_value(
                            py, package, descriptor, field_name, item,
                        )?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            lowered.push(DynamicExpr::Or { exprs });
            continue;
        }
        let operator = match lookup {
            "eq" | "exact" => DynamicComparisonOp::Eq,
            "ne" => DynamicComparisonOp::Neq,
            "gt" => DynamicComparisonOp::Gt,
            "gte" => DynamicComparisonOp::Gte,
            "lt" => DynamicComparisonOp::Lt,
            "lte" => DynamicComparisonOp::Lte,
            "contains" => DynamicComparisonOp::Contains,
            "startswith" => DynamicComparisonOp::StartsWith,
            "endswith" => DynamicComparisonOp::EndsWith,
            "regex" | "like" => DynamicComparisonOp::Like,
            _ => {
                return Err(py_value_error(format!(
                    "unsupported generated manager lookup {lookup:?}; expected exact, eq, ne, gt, gte, lt, lte, contains, startswith, endswith, regex, in, or isnull"
                )));
            }
        };
        lowered.push(DynamicExpr::Compare {
            attr_name: descriptor.attr_name.clone(),
            operator,
            value: projected_filter_attribute_value(py, package, descriptor, field_name, &value)?,
        });
    }
    Ok(lowered)
}

fn projected_filter_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    field_name: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<AttributeValue> {
    let expected = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
    let expected_class = package.class(expected, ProjectedModelForm::Complete)?;
    if value.get_type().as_ptr() == expected_class.bind(py).as_ptr() {
        let scalar = value.call_method0("runtime_attribute_value")?;
        attribute_value_from_py(py, &scalar, descriptor.value_type)
    } else if package.identify_value(py, value).is_ok() {
        Err(py_type_error(format!(
            "generated manager filter {field_name:?} requires its exact attribute wrapper"
        )))
    } else {
        attribute_value_from_py(py, value, descriptor.value_type)
    }
}

fn projected_filter_items<'py>(
    value: &Bound<'py, PyAny>,
    lookup: &str,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    if value.cast::<PyString>().is_ok() || value.cast::<PyDict>().is_ok() {
        return Err(py_type_error(format!(
            "generated manager {lookup} lookup requires a non-string iterable"
        )));
    }
    let items = value
        .try_iter()
        .map_err(|_| {
            py_type_error(format!(
                "generated manager {lookup} lookup requires an iterable"
            ))
        })?
        .collect::<PyResult<Vec<_>>>()?;
    if items.is_empty() {
        return Err(py_value_error(format!(
            "generated manager {lookup} lookup requires at least one value"
        )));
    }
    Ok(items)
}

fn projected_filter_iid(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let iid = value
        .cast::<PyString>()
        .map_err(|_| py_type_error("generated manager IID lookup requires strings"))?
        .to_str()?
        .to_owned();
    if !is_canonical_thing_iid(&iid) {
        return Err(py_value_error(
            "generated manager IID lookup requires a canonical TypeDB thing IID",
        ));
    }
    Ok(iid)
}

fn lower_roles(
    py: Python<'_>,
    package: &InstalledPackage,
    relation_id: &TypeId,
    descriptor: &RelationDescriptor,
    instance: &Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicRolePlayerInput>> {
    let projection = package.projection.projection();
    let model = &projection.models()[relation_id];
    let values = instance.call_method0("runtime_values")?;
    let values = values.cast::<PyDict>()?;
    let mut inputs = Vec::new();
    for create in model.create().roles().values() {
        let token = &model.query_tokens().roles()[create.role()];
        let role_name = create.role().label().as_str();
        let role = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("projected role has no provider descriptor"))?;
        let value = values.get_item(token.target_name().as_str())?;
        for item in normalized_items(value.as_ref(), role_cardinality(role))? {
            let (player_id, form) = package.identify_value(py, &item)?;
            if !create
                .players()
                .iter()
                .any(|allowed| allowed.id() == &player_id && allowed.form() == form)
            {
                return Err(py_type_error(format!(
                    "role {:?} received an incompatible projected player",
                    token.target_name().as_str()
                )));
            }
            let iid = projected_iid(&item)?;
            let key = if iid.is_none() {
                projected_key(py, package, &player_id, &item)?
            } else {
                None
            };
            if iid.is_none() && key.is_none() {
                return Err(py_value_error(format!(
                    "role player {:?} requires an attached IID or projected key",
                    player_id.label().as_str()
                )));
            }
            inputs.push(DynamicRolePlayerInput {
                role_name: role_name.to_owned(),
                player_type_name: player_id.label().as_str().to_owned(),
                iid,
                key,
            });
        }
    }
    Ok(inputs)
}

fn projected_key(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    value: &Bound<'_, PyAny>,
) -> PyResult<Option<(String, AttributeValue)>> {
    let descriptor = match package.projection.descriptor(id) {
        Ok(TypeDescriptor::Entity(descriptor)) => descriptor,
        Ok(TypeDescriptor::Relation(_)) | Err(_) => return Ok(None),
    };
    let Some(key) = descriptor.key_attribute() else {
        return Ok(None);
    };
    let values = value.call_method0("runtime_values")?;
    let values = values.cast::<PyDict>()?;
    let Some(wrapper) = values.get_item(&key.field_name)? else {
        return Ok(None);
    };
    if wrapper.is_none() {
        return Ok(None);
    }
    let (wrapper_id, form) = package.identify_value(py, &wrapper)?;
    let expected = package.type_by_label(&key.attr_name, TypeKind::Attribute)?;
    if &wrapper_id != expected || form != ProjectedModelForm::Complete {
        return Err(py_type_error(
            "projected key uses the wrong attribute wrapper",
        ));
    }
    let scalar = wrapper.call_method0("runtime_attribute_value")?;
    Ok(Some((
        key.attr_name.clone(),
        attribute_value_from_py(py, &scalar, key.value_type)?,
    )))
}

fn projected_iid(value: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    let iid = value.getattr("iid")?;
    if iid.is_none() {
        Ok(None)
    } else {
        iid.extract().map(Some)
    }
}

fn projected_iid_from_snapshot(
    py: Python<'_>,
    snapshot: &ProjectedFacadeSnapshot,
    ordinal: usize,
) -> PyResult<String> {
    let iid = snapshot.iid.bind(py);
    if iid.is_none() {
        return Ok(String::new());
    }
    let iid = iid.cast_exact::<PyString>().map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "batch_iid_type_mismatch",
            "Successor update rows require an exact string IID slot",
            &projected_batch_iid_path(ordinal),
        ))
    })?;
    let iid = iid.to_str().map_err(|_| {
        py_sdk_diagnostic(batch_input_shape_diagnostic(
            "batch_iid_type_mismatch",
            "Successor update rows require a UTF-8 string IID slot",
            &projected_batch_iid_path(ordinal),
        ))
    })?;
    copy_canonical_batch_iid(iid, ordinal)
}

fn copy_canonical_batch_iid(iid: &str, ordinal: usize) -> PyResult<String> {
    copy_canonical_iid_at(
        iid,
        &projected_batch_iid_path(ordinal),
        "The batch target IID is not canonical TypeDB identity text",
    )
}

fn copy_canonical_iid_at(
    iid: &str,
    path: &[SdkDiagnosticPathSegment],
    message: &'static str,
) -> PyResult<String> {
    if !is_canonical_thing_iid(iid) {
        let diagnostic = path.iter().cloned().fold(
            SdkExecutionDiagnostic::invalid_input(
                SdkDiagnosticCode::new("noncanonical_iid")
                    .expect("the static noncanonical-IID code is canonical"),
                SdkDiagnosticMessage::new(message)
                    .expect("the static noncanonical-IID message is canonical"),
            ),
            |diagnostic, segment| {
                diagnostic
                    .try_at(segment)
                    .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
            },
        );
        return Err(py_sdk_diagnostic(diagnostic));
    }
    let mut retained = String::new();
    retained
        .try_reserve_exact(iid.len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    retained.push_str(iid);
    Ok(retained)
}

fn required_projected_iid(value: &Bound<'_, PyAny>) -> PyResult<String> {
    projected_iid(value)?.ok_or_else(|| {
        py_value_error("generated manager update and delete require an attached TypeDB IID")
    })
}

fn resolved_entity_iid(row: Option<&DynamicEntityRow>) -> PyResult<Option<String>> {
    row.map(|row| {
        row.iid
            .clone()
            .ok_or_else(|| py_runtime_error("generated entity identity lookup omitted its IID"))
    })
    .transpose()
}

fn resolved_relation_iid(row: Option<&DynamicRelationRow>) -> PyResult<Option<String>> {
    row.map(|row| {
        row.iid
            .clone()
            .ok_or_else(|| py_runtime_error("generated relation identity lookup omitted its IID"))
    })
    .transpose()
}

fn normalized_items<'py>(
    value: Option<&Bound<'py, PyAny>>,
    cardinality: (u32, Option<u32>),
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let (minimum, maximum) = cardinality;
    let mut items = Vec::new();
    match value {
        None => {}
        Some(value) if value.is_none() => {}
        Some(value) if maximum == Some(1) => items.push(value.clone()),
        Some(value) => {
            if value.cast::<PyString>().is_ok() {
                return Err(py_type_error(
                    "projected multi-value input requires a sequence",
                ));
            }
            let tuple = value
                .cast::<PyTuple>()
                .map_err(|_| py_type_error("projected multi-value input requires a tuple"))?;
            items.extend(tuple.iter());
        }
    }
    let count = u32::try_from(items.len())
        .map_err(|_| py_value_error("projected value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_value_error(
            "projected value violates resolved cardinality",
        ));
    }
    Ok(items)
}

fn descriptor_cardinality(descriptor: &OwnedAttributeDescriptor) -> (u32, Option<u32>) {
    descriptor
        .cardinality()
        .unwrap_or((u32::from(!descriptor.is_optional), Some(1)))
}

fn role_cardinality(descriptor: &RoleDescriptor) -> (u32, Option<u32>) {
    descriptor.cardinality.unwrap_or((0, Some(1)))
}

fn hydrate_projected_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: Arc<ProjectedThing>,
) -> PyResult<Py<PyAny>> {
    let mut pending_origins = pending_projected_origins(projected.as_ref())?;
    let instance = hydrate_projected_thing_value_staged(
        py,
        package,
        projected.as_ref(),
        None,
        &mut pending_origins,
        false,
        None,
    )?;
    let origin = package.facade_origins.prepare(instance.bind(py))?;
    for pending in pending_origins {
        package
            .facade_origins
            .install(pending.origin, pending.proof);
    }
    package
        .facade_origins
        .install(origin, FacadeProjectionProof::Thing(projected));
    Ok(instance)
}

fn hydrate_projected_thing_value(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: &ProjectedThing,
) -> PyResult<Py<PyAny>> {
    let mut pending_origins = pending_projected_origins(projected)?;
    let instance = hydrate_projected_thing_value_staged(
        py,
        package,
        projected,
        None,
        &mut pending_origins,
        false,
        None,
    )?;
    for pending in pending_origins {
        package
            .facade_origins
            .install(pending.origin, pending.proof);
    }
    let origin = package.facade_origins.prepare(instance.bind(py))?;
    package
        .facade_origins
        .install(origin, FacadeProjectionProof::DetachedSnapshot);
    Ok(instance)
}

fn pending_projected_origins(projected: &ProjectedThing) -> PyResult<Vec<PendingFacadeOrigin>> {
    let capacity = projected
        .roles()
        .values()
        .try_fold(0_usize, |count, players| count.checked_add(players.len()))
        .ok_or_else(|| py_runtime_error("projected role-player count exceeds usize"))?;
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(capacity)
        .map_err(|_| py_runtime_error("projected role-player origin allocation failed"))?;
    Ok(pending)
}

fn hydrate_projected_thing_value_staged(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: &ProjectedThing,
    parent_proof: Option<&Arc<ProjectedThing>>,
    pending_origins: &mut Vec<PendingFacadeOrigin>,
    callback_free: bool,
    pool: Option<&BatchHydrationPool>,
) -> PyResult<Py<PyAny>> {
    if !callback_free {
        projected
            .validate_for(package.projection.as_ref())
            .map_err(py_sdk_diagnostic)?;
    }
    let id = projected.type_id();
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_runtime_error(
            "projected thing hydration requires an entity or relation",
        ));
    }
    let values = hydrate_projected_fields(
        py,
        package,
        id,
        HydratedProjectedFields::Complete(projected.fields()),
        callback_free,
        pool,
    )?;
    if id.kind() == TypeKind::Relation {
        let model = package
            .projection
            .projection()
            .models()
            .get(id)
            .ok_or_else(|| py_runtime_error("projected relation model is absent"))?;
        for (role_ordinal, (role_id, read)) in model.complete_read().roles().iter().enumerate() {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                py_runtime_error("projected relation read role has no query token")
            })?;
            let players = projected.roles().get(role_id).ok_or_else(|| {
                py_runtime_error("projected relation omitted a validated read role")
            })?;
            let mut hydrated = Vec::new();
            hydrated
                .try_reserve(players.len())
                .map_err(|_| py_runtime_error("projected relation role allocation failed"))?;
            for (player_ordinal, player) in players.iter().enumerate() {
                hydrated.push(hydrate_projected_player(
                    py,
                    package,
                    player,
                    parent_proof,
                    role_ordinal,
                    player_ordinal,
                    pending_origins,
                    callback_free,
                    pool,
                )?);
            }
            set_projected_hydrated_values(
                py,
                &values,
                token.target_name().as_str(),
                hydrated,
                read.multiplicity(),
            )?;
        }
    } else if !projected.roles().is_empty() {
        return Err(py_runtime_error(
            "projected entity unexpectedly contains relation roles",
        ));
    }
    if callback_free {
        allocate_projected_complete_callback_free(
            py,
            package,
            id,
            &values,
            projected.iid(),
            pool.expect("successor callback-free hydration retains its outer pool"),
        )
    } else {
        allocate_projected_complete(py, package, id, &values, projected.iid())
    }
}

#[allow(clippy::too_many_arguments)]
fn hydrate_projected_player(
    py: Python<'_>,
    package: &InstalledPackage,
    player: &ProjectedRolePlayer,
    parent_proof: Option<&Arc<ProjectedThing>>,
    role_ordinal: usize,
    player_ordinal: usize,
    pending_origins: &mut Vec<PendingFacadeOrigin>,
    callback_free: bool,
    pool: Option<&BatchHydrationPool>,
) -> PyResult<Py<PyAny>> {
    if !callback_free {
        player
            .validate_for(package.projection.as_ref())
            .map_err(py_sdk_diagnostic)?;
    }
    let instance = match player.exact_form() {
        Some(ProjectedModelForm::Complete) => {
            if player.type_id().kind() != TypeKind::Entity {
                return Err(py_sdk_diagnostic(projected_role_player_form_missing(
                    player.type_id(),
                )));
            }
            let values = hydrate_projected_fields(
                py,
                package,
                player.type_id(),
                HydratedProjectedFields::Complete(player.fields()),
                callback_free,
                pool,
            )?;
            if callback_free {
                allocate_projected_complete_callback_free(
                    py,
                    package,
                    player.type_id(),
                    &values,
                    player.iid(),
                    pool.expect("successor callback-free hydration retains its outer pool"),
                )?
            } else {
                allocate_projected_complete(py, package, player.type_id(), &values, player.iid())?
            }
        }
        Some(ProjectedModelForm::Reference) => {
            let values = hydrate_projected_fields(
                py,
                package,
                player.type_id(),
                HydratedProjectedFields::Reference(player.keys()),
                callback_free,
                pool,
            )?;
            if callback_free {
                allocate_projected_reference_callback_free(
                    py,
                    package,
                    player.type_id(),
                    &values,
                    player.iid(),
                    pool.expect("successor callback-free hydration retains its outer pool"),
                )?
            } else {
                allocate_projected_reference(py, package, player.type_id(), &values, player.iid())?
            }
        }
        None => {
            return Err(py_sdk_diagnostic(projected_role_player_form_missing(
                player.type_id(),
            )));
        }
    };
    let proof = if callback_free {
        FacadeProjectionProof::RolePlayer {
            parent: Arc::clone(parent_proof.ok_or_else(|| {
                py_runtime_error("successor nested hydration omitted its retained parent proof")
            })?),
            role_ordinal,
            player_ordinal,
        }
    } else {
        FacadeProjectionProof::Reference(Arc::new(player.reference().clone()))
    };
    pending_origins.push(PendingFacadeOrigin {
        origin: package.facade_origins.prepare(instance.bind(py))?,
        proof,
    });
    Ok(instance)
}

#[derive(Clone, Copy)]
enum HydratedProjectedFields<'a> {
    Complete(&'a BTreeMap<type_bridge_contract::schema::OwnsFactId, Vec<ProjectedAttributeValue>>),
    Reference(&'a BTreeMap<type_bridge_contract::schema::OwnsFactId, ProjectedAttributeValue>),
}

impl HydratedProjectedFields<'_> {
    fn len(self) -> usize {
        match self {
            Self::Complete(fields) => fields.len(),
            Self::Reference(fields) => fields.len(),
        }
    }

    fn is_reference(self) -> bool {
        matches!(self, Self::Reference(_))
    }
}

fn hydrate_projected_fields<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    id: &TypeId,
    fields: HydratedProjectedFields<'_>,
    callback_free: bool,
    pool: Option<&BatchHydrationPool>,
) -> PyResult<Bound<'py, PyDict>> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projected hydrated model is absent"))?;
    let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
        TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
    };
    let reference = fields.is_reference();
    let selected_count = model
        .complete_read()
        .fields()
        .iter()
        .filter(|field| !reference || model.reference_read().key_fields().contains(field.token()))
        .count();
    if fields.len() != selected_count {
        return Err(py_runtime_error(
            "projected hydrated fields do not match the selected model form",
        ));
    }
    let values = if callback_free {
        fallible_python_dict(py)?
    } else {
        PyDict::new(py)
    };
    for read in
        model.complete_read().fields().iter().filter(|field| {
            !reference || model.reference_read().key_fields().contains(field.token())
        })
    {
        let field_id = read.token();
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| py_runtime_error("projected hydrated field has no query token"))?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == token.target_name().as_str()
                    && descriptor.attr_name == field_id.attribute().label().as_str()
            })
            .ok_or_else(|| py_runtime_error("projected hydrated field has no descriptor"))?;
        let projected_values_len = match fields {
            HydratedProjectedFields::Complete(fields) => {
                fields.get(field_id).map(Vec::len).ok_or_else(|| {
                    py_runtime_error("projected hydrated model omitted a selected field")
                })?
            }
            HydratedProjectedFields::Reference(fields) => {
                usize::from(fields.contains_key(field_id))
            }
        };
        if projected_values_len == 0 && matches!(fields, HydratedProjectedFields::Reference(_)) {
            return Err(py_runtime_error(
                "projected hydrated model omitted a selected field",
            ));
        }
        let mut hydrated = Vec::new();
        hydrated
            .try_reserve(projected_values_len)
            .map_err(|_| py_runtime_error("projected hydrated field allocation failed"))?;
        let mut hydrate_value = |value: &ProjectedAttributeValue| -> PyResult<()> {
            if value.attribute_type().label().as_str() != descriptor.attr_name {
                return Err(py_runtime_error(
                    "projected hydrated scalar has the wrong attribute type",
                ));
            }
            hydrated.push(if callback_free {
                hydrate_projected_attribute_callback_free(
                    py,
                    package,
                    descriptor,
                    value,
                    pool.expect("successor callback-free hydration retains its outer pool"),
                )?
            } else {
                hydrate_attribute(py, package, descriptor, &value.to_attribute_value())?
            });
            Ok(())
        };
        match fields {
            HydratedProjectedFields::Complete(fields) => {
                for value in fields.get(field_id).ok_or_else(|| {
                    py_runtime_error("projected hydrated model omitted a selected field")
                })? {
                    hydrate_value(value)?;
                }
            }
            HydratedProjectedFields::Reference(fields) => {
                hydrate_value(fields.get(field_id).ok_or_else(|| {
                    py_runtime_error("projected hydrated model omitted a selected field")
                })?)?;
            }
        }
        set_projected_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            hydrated,
            read.multiplicity(),
        )?;
    }
    Ok(values)
}

fn set_projected_hydrated_values(
    py: Python<'_>,
    values: &Bound<'_, PyDict>,
    name: &str,
    items: Vec<Py<PyAny>>,
    multiplicity: ProjectedMultiplicity,
) -> PyResult<()> {
    let cardinality = multiplicity.cardinality();
    let count = u64::try_from(items.len())
        .map_err(|_| py_runtime_error("projected hydrated value count exceeds u64"))?;
    if count < cardinality.min() || cardinality.max().is_some_and(|maximum| count > maximum) {
        return Err(py_runtime_error(
            "projected hydrated value violates its projected cardinality",
        ));
    }
    let name = fallible_python_string(py, name)?;
    match multiplicity.container() {
        ProjectedContainer::Scalar => match items.into_iter().next() {
            Some(value) => values.set_item(name.bind(py), value),
            None => values.set_item(name.bind(py), py.None()),
        },
        ProjectedContainer::Sequence => values.set_item(name.bind(py), PyTuple::new(py, items)?),
    }
}

fn allocate_projected_complete(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
) -> PyResult<Py<PyAny>> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Complete)?)?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    instance.call_method1("attach_runtime_iid", (iid,))?;
    Ok(instance.unbind())
}

fn allocate_projected_complete_callback_free(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
    pool: &BatchHydrationPool,
) -> PyResult<Py<PyAny>> {
    let slots = package.batch_slots_for(id, ProjectedModelForm::Complete)?;
    let iid = fallible_python_string(py, iid)?;
    slots.allocate_initialized_after_fence(py, values.as_any(), iid.bind(py), None, pool)
}

fn allocate_projected_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
) -> PyResult<Py<PyAny>> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Reference)?)?;
    instance.call_method1("initialize_runtime_reference", (iid, values))?;
    Ok(instance.unbind())
}

fn allocate_projected_detached_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Reference)?)?;
    instance.call_method1("initialize_runtime_reference", (iid, values))?;
    Ok(instance.unbind())
}

fn hydrate_projected_detached_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: &ProjectedReference,
) -> PyResult<Py<PyAny>> {
    let values = hydrate_projected_fields(
        py,
        package,
        projected.type_id(),
        HydratedProjectedFields::Reference(projected.keys()),
        false,
        None,
    )?;
    allocate_projected_detached_reference(
        py,
        package,
        projected.type_id(),
        &values,
        projected.iid(),
    )
}

fn allocate_projected_reference_callback_free(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
    pool: &BatchHydrationPool,
) -> PyResult<Py<PyAny>> {
    let slots = package.batch_slots_for(id, ProjectedModelForm::Reference)?;
    let iid = fallible_python_string(py, iid)?;
    slots.allocate_initialized_after_fence(py, values.as_any(), iid.bind(py), None, pool)
}

fn projected_role_player_form_missing(type_id: &TypeId) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(
        SdkDiagnosticCode::new("hydrated_role_player_form_missing")
            .expect("static projected-role code is canonical"),
        SdkDiagnosticMessage::new(
            "Successor hydration requires exact complete-or-reference role-player evidence",
        )
        .expect("static projected-role message is canonical"),
    )
    .try_at(SdkDiagnosticPathSegment::Type(type_id.clone()))
    .expect("a projected role-player type path is bounded")
}

fn hydrate_validated_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
) -> PyResult<Py<PyAny>> {
    if let Some(projected) = handle.projected()? {
        return hydrate_projected_thing(py, package, projected);
    }
    let thing = handle.hydrated()?;
    let label = handle.descriptor_type_name(thing.concrete_descriptor())?;
    let id = package
        .types_by_label
        .get(&label)
        .ok_or_else(|| py_runtime_error("query result type is outside the installed projection"))?;
    match thing.kind() {
        ThingKind::Entity if id.kind() == TypeKind::Entity => {
            let descriptor = package
                .projection
                .entity_descriptor(id)
                .map_err(py_orm_error)?;
            let values = hydrate_validated_attributes(
                py,
                package,
                &descriptor.owned_attributes,
                thing.attributes(),
            )?;
            hydrate_complete(py, package, id, &values, Some(thing.concept_id().as_str()))
        }
        ThingKind::Relation if id.kind() == TypeKind::Relation => {
            hydrate_validated_relation(py, package, handle, id, thing)
        }
        _ => Err(py_runtime_error(
            "query result kind conflicts with its installed projection type",
        )),
    }
}

fn hydrate_validated_relation(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
    id: &TypeId,
    thing: &HydratedThing,
) -> PyResult<Py<PyAny>> {
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_validated_attributes(
        py,
        package,
        &descriptor.owned_attributes,
        thing.attributes(),
    )?;
    let model = &package.projection.projection().models()[id];
    for read in model.complete_read().roles().values() {
        let token = &model.query_tokens().roles()[read.role()];
        let role_name = read.role().label().as_str();
        let role_descriptor = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("query result role has no provider descriptor"))?;
        let mut players = Vec::new();
        if let Some(role) = thing
            .roles()
            .iter()
            .find(|role| role.role().name == role_name)
        {
            for player in role.players() {
                players.push(hydrate_validated_player(
                    py,
                    package,
                    handle,
                    read.players(),
                    player,
                )?);
            }
        }
        set_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            players,
            role_cardinality(role_descriptor),
        )?;
    }
    hydrate_complete(py, package, id, &values, Some(thing.concept_id().as_str()))
}

fn hydrate_validated_player(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &HydratedRolePlayer,
) -> PyResult<Py<PyAny>> {
    let label = handle.descriptor_type_name(player.concrete_descriptor())?;
    let id = package
        .types_by_label
        .get(&label)
        .ok_or_else(|| py_runtime_error("query role-player type is outside the projection"))?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| {
            py_runtime_error("query role player is not accepted by the projected role")
        })?;
    let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
        TypeDescriptor::Entity(descriptor) => match projected.form() {
            ProjectedModelForm::Complete => descriptor.owned_attributes.clone(),
            ProjectedModelForm::Reference => descriptor
                .owned_attributes
                .iter()
                .filter(|attribute| attribute.is_key())
                .cloned()
                .collect(),
        },
        TypeDescriptor::Relation(_) => {
            if projected.form() == ProjectedModelForm::Complete {
                return Err(py_runtime_error(
                    "nested complete relation query hydration is forbidden",
                ));
            }
            Vec::new()
        }
    };
    let values = hydrate_validated_attributes(py, package, &descriptors, player.attributes())?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            hydrate_complete(py, package, id, &values, Some(player.concept_id().as_str()))
        }
        ProjectedModelForm::Reference => {
            hydrate_reference(py, package, id, &values, player.concept_id().as_str())
        }
    }
}

fn hydrate_validated_attributes<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &[HydratedAttribute],
) -> PyResult<Bound<'py, PyDict>> {
    let values = PyDict::new(py);
    for descriptor in descriptors {
        let mut wrappers = Vec::new();
        if let Some(attribute) = attributes
            .iter()
            .find(|attribute| attribute.field().name == descriptor.field_name)
        {
            for value in attribute.values() {
                wrappers.push(hydrate_attribute(py, package, descriptor, value)?);
            }
        }
        set_hydrated_values(
            py,
            &values,
            &descriptor.field_name,
            wrappers,
            descriptor_cardinality(descriptor),
        )?;
    }
    Ok(values)
}

fn hydrate_entity(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicEntityRow,
) -> PyResult<Py<PyAny>> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .entity_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_attributes(py, package, &descriptor.owned_attributes, &row.attributes)?;
    hydrate_complete(py, package, id, &values, row.iid.as_deref())
}

fn replace_projected_instance(
    py: Python<'_>,
    instance: Bound<'_, PyAny>,
    hydrated: Py<PyAny>,
) -> PyResult<Py<PyAny>> {
    let stored = hydrated.bind(py);
    let iid = required_projected_iid(stored)?;
    let values = stored.call_method0("runtime_values")?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    instance.call_method1("attach_runtime_iid", (iid,))?;
    Ok(instance.unbind())
}

fn snapshot_projected_instance(instance: &Bound<'_, PyAny>) -> PyResult<ProjectedFacadeSnapshot> {
    Ok(ProjectedFacadeSnapshot {
        iid: instance.getattr("_iid")?.unbind(),
        values: instance.getattr("_values")?.unbind(),
    })
}

fn replace_projected_instance_atomic(
    py: Python<'_>,
    instance: &Bound<'_, PyAny>,
    hydrated: Py<PyAny>,
    snapshot: &ProjectedFacadeSnapshot,
) -> PyResult<()> {
    if let Err(error) = replace_projected_instance(py, instance.clone(), hydrated) {
        if let Err(rollback) = restore_projected_instance(instance, snapshot) {
            return Err(py_runtime_error(format!(
                "projected facade replacement failed and rollback failed: {error}; {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn restore_projected_instance(
    instance: &Bound<'_, PyAny>,
    snapshot: &ProjectedFacadeSnapshot,
) -> PyResult<()> {
    let py = instance.py();
    let values_error = instance.setattr("_values", snapshot.values.bind(py)).err();
    let iid_error = instance.setattr("_iid", snapshot.iid.bind(py)).err();
    match (values_error, iid_error) {
        (None, None) => Ok(()),
        (Some(error), None) | (None, Some(error)) => Err(error),
        (Some(values), Some(iid)) => Err(py_runtime_error(format!(
            "projected facade values and IID rollback both failed: {values}; {iid}"
        ))),
    }
}

fn hydrate_relation(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicRelationRow,
) -> PyResult<Py<PyAny>> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_attributes(py, package, &descriptor.owned_attributes, &row.attributes)?;
    let projection = package.projection.projection();
    let model = &projection.models()[id];
    for read in model.complete_read().roles().values() {
        let token = &model.query_tokens().roles()[read.role()];
        let role_name = read.role().label().as_str();
        let role = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("read role has no provider descriptor"))?;
        let mut players = Vec::new();
        for player in row
            .role_players
            .iter()
            .filter(|player| player.role_name == role_name)
        {
            players.push(hydrate_player(py, package, read.players(), player)?);
        }
        set_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            players,
            role_cardinality(role),
        )?;
    }
    hydrate_complete(py, package, id, &values, row.iid.as_deref())
}

fn hydrate_player(
    py: Python<'_>,
    package: &InstalledPackage,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &DynamicRolePlayer,
) -> PyResult<Py<PyAny>> {
    let label = player
        .player_type_name
        .as_deref()
        .ok_or_else(|| py_runtime_error("role-player row has no concrete type label"))?;
    let id = package
        .types_by_label
        .get(label)
        .ok_or_else(|| py_runtime_error("role-player row type is outside the projection"))?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| {
            py_runtime_error("role-player row type is not accepted by the projected role")
        })?;
    let attributes = package
        .projection
        .role_player_attributes(id, &player.attributes)
        .map_err(py_orm_error)?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            if id.kind() != TypeKind::Entity {
                return Err(py_runtime_error(
                    "nested complete relation hydration is forbidden; use its reference projection",
                ));
            }
            let row = DynamicEntityRow {
                iid: player.player_iid.clone(),
                type_name: player.player_type_name.clone(),
                attributes,
            };
            hydrate_entity(py, package, id, &row)
        }
        ProjectedModelForm::Reference => {
            let iid = player
                .player_iid
                .as_deref()
                .ok_or_else(|| py_runtime_error("reference role-player row has no IID"))?;
            let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
                TypeDescriptor::Entity(descriptor) => descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .cloned()
                    .collect::<Vec<_>>(),
                TypeDescriptor::Relation(_) => Vec::new(),
            };
            let values = hydrate_attributes(py, package, &descriptors, &attributes)?;
            hydrate_reference(py, package, id, &values, iid)
        }
    }
}

fn hydrate_attributes<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &DynamicAttributeMap,
) -> PyResult<Bound<'py, PyDict>> {
    let values = PyDict::new(py);
    for descriptor in descriptors {
        let mut wrappers = Vec::new();
        for (_, value) in attributes
            .iter()
            .filter(|(name, _)| name == &descriptor.attr_name)
        {
            wrappers.push(hydrate_attribute(py, package, descriptor, value)?);
        }
        set_hydrated_values(
            py,
            &values,
            &descriptor.field_name,
            wrappers,
            descriptor_cardinality(descriptor),
        )?;
    }
    Ok(values)
}

fn hydrate_attribute(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    value: &AttributeValue,
) -> PyResult<Py<PyAny>> {
    ensure_attribute_type(value, descriptor.value_type)?;
    let id = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
    if projection_uses_ordered_collections(package.projection.projection()) {
        ProjectedAttributeValue::try_from_hydrated_attribute_value(
            &package.projection,
            id.clone(),
            value,
        )
        .map_err(py_sdk_diagnostic)?;
    }
    let class = package.class(id, ProjectedModelForm::Complete)?;
    let scalar = attribute_value_to_py(py, value, package.named_zone_marker.as_ref())?;
    class.bind(py).call1((scalar,)).map(Bound::unbind)
}

fn hydrate_projected_attribute_callback_free(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    value: &ProjectedAttributeValue,
    pool: &BatchHydrationPool,
) -> PyResult<Py<PyAny>> {
    let id = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
    let projection = package.projection.projection();
    if value.attribute_type() != id
        || projected_value_type(value.value().value_type()) != descriptor.value_type
        || value.semantic_fingerprint() != projection.semantic_fingerprint()
        || value.binding_target() != projection.target()
        || value.projection_fingerprint() != projection.projection_fingerprint()
    {
        return Err(py_runtime_error(
            "projected hydrated scalar has the wrong attribute type or projection brand",
        ));
    }
    let scalar_hydration = package
        .scalar_hydration
        .as_ref()
        .ok_or_else(|| py_runtime_error("successor scalar hydration plan was not installed"))?;
    let scalar = scalar_hydration.projected_to_py(
        py,
        value.value(),
        package.named_zone_slots.as_ref(),
        pool,
    )?;
    let values = fallible_python_dict(py)?;
    let iid = py.None();
    package
        .batch_slots_for(id, ProjectedModelForm::Complete)?
        .allocate_initialized_after_fence(
            py,
            values.as_any(),
            iid.bind(py),
            Some(scalar.bind(py)),
            pool,
        )
}

fn set_hydrated_values(
    py: Python<'_>,
    values: &Bound<'_, PyDict>,
    name: &str,
    items: Vec<Py<PyAny>>,
    cardinality: (u32, Option<u32>),
) -> PyResult<()> {
    let (minimum, maximum) = cardinality;
    let count = u32::try_from(items.len())
        .map_err(|_| py_value_error("hydrated value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_runtime_error(
            "provider row violates projected cardinality",
        ));
    }
    if maximum == Some(1) {
        match items.into_iter().next() {
            Some(value) => values.set_item(name, value)?,
            None => values.set_item(name, py.None())?,
        }
    } else {
        values.set_item(name, PyTuple::new(py, items)?)?;
    }
    Ok(())
}

fn hydrate_complete(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: Option<&str>,
) -> PyResult<Py<PyAny>> {
    if projection_uses_ordered_collections(package.projection.projection()) {
        project_hydrated_thing(py, package, id, values, iid)?;
    }
    let instance = allocate(py, package.class(id, ProjectedModelForm::Complete)?)?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    if let Some(iid) = iid {
        instance.call_method1("attach_runtime_iid", (iid,))?;
    }
    Ok(instance.unbind())
}

fn hydrate_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
) -> PyResult<Py<PyAny>> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Reference)?)?;
    instance.call_method1("initialize_runtime_reference", (iid, values))?;
    Ok(instance.unbind())
}

fn allocate<'py>(py: Python<'py>, class: &Py<PyType>) -> PyResult<Bound<'py, PyAny>> {
    let class = class.bind(py);
    class.getattr("__new__")?.call1((class,))
}

fn ensure_row_type(id: &TypeId, actual: Option<&str>) -> PyResult<()> {
    if actual.is_some_and(|actual| actual != id.label().as_str()) {
        return Err(py_runtime_error(
            "exact provider row returned a different concrete type",
        ));
    }
    Ok(())
}

fn attribute_value_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
) -> PyResult<AttributeValue> {
    match value_type {
        ValueType::String => value
            .cast_exact::<PyString>()
            .map_err(|_| py_type_error("attribute value requires an exact str"))?
            .extract()
            .map(AttributeValue::String),
        ValueType::Long => value
            .cast_exact::<PyInt>()
            .map_err(|_| py_type_error("attribute value requires an exact int"))?
            .extract()
            .map(AttributeValue::Long),
        ValueType::Double => value
            .cast_exact::<PyFloat>()
            .map_err(|_| py_type_error("attribute value requires an exact float"))?
            .extract()
            .map(AttributeValue::Double),
        ValueType::Boolean => value
            .cast_exact::<PyBool>()
            .map_err(|_| py_type_error("attribute value requires an exact bool"))?
            .extract()
            .map(AttributeValue::Boolean),
        ValueType::Date => {
            exact_temporal_string(py, value, "date", false).map(AttributeValue::Date)
        }
        ValueType::DateTime => {
            exact_temporal_string(py, value, "datetime", false).map(AttributeValue::DateTime)
        }
        ValueType::DateTimeTz => {
            exact_temporal_string(py, value, "datetime", true).map(AttributeValue::DateTimeTZ)
        }
        ValueType::Decimal => {
            exact_module_value_string(py, value, "decimal", "Decimal").map(AttributeValue::Decimal)
        }
        ValueType::Duration => duration_from_py(py, value).map(AttributeValue::Duration),
    }
}

fn canonical_attribute_value_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<AttributeValue> {
    canonical_attribute_value_from_py_at(py, value, value_type, named_zone_marker, &[])
}

fn canonical_projected_value_from_py_at(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
    hydration: &ProjectedScalarHydration,
    named_zone_marker: Option<&Py<PyType>>,
    named_zone_slots: Option<&ProjectedNamedZoneSlots>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<CanonicalValue> {
    let wrong = || py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path));
    match value_type {
        ValueType::String => {
            let text = value.cast_exact::<PyString>().map_err(|_| wrong())?;
            let retained =
                copy_projected_text(text, MAX_CANONICAL_STRING_BYTES, |_| true, operation_path)?;
            CanonicalString::new(retained)
                .map(CanonicalValue::String)
                .map_err(|_| wrong())
        }
        ValueType::Long => value
            .cast_exact::<PyInt>()
            .map_err(|_| wrong())?
            .extract::<i64>()
            .map(CanonicalValue::Long)
            .map_err(|_| wrong()),
        ValueType::Double => {
            let value = value
                .cast_exact::<PyFloat>()
                .map_err(|_| wrong())?
                .extract::<f64>()
                .map_err(|_| wrong())?;
            CanonicalDouble::new(value)
                .map(CanonicalValue::Double)
                .map_err(|_| wrong())
        }
        ValueType::Boolean => value
            .cast_exact::<PyBool>()
            .map_err(|_| wrong())?
            .extract::<bool>()
            .map(CanonicalValue::Boolean)
            .map_err(|_| wrong()),
        ValueType::Date => canonical_projected_date(py, value, hydration)
            .map(CanonicalValue::Date)
            .map_err(|_| wrong()),
        ValueType::DateTime => {
            let local = canonical_projected_datetime(py, value, hydration).map_err(|_| wrong())?;
            let offset = projected_datetime_offset(py, value).map_err(|_| wrong())?;
            if !offset.is_none() {
                return Err(wrong());
            }
            Ok(CanonicalValue::DateTime(local))
        }
        ValueType::DateTimeTz => canonical_projected_datetime_tz(
            py,
            value,
            hydration,
            named_zone_marker,
            named_zone_slots,
            operation_path,
        )
        .map(CanonicalValue::DateTimeTz),
        ValueType::Decimal => canonical_projected_decimal(py, value, hydration, operation_path)
            .map(CanonicalValue::Decimal),
        ValueType::Duration => canonical_projected_duration(py, value, hydration)
            .map(CanonicalValue::Duration)
            .map_err(|_| wrong()),
    }
}

fn copy_projected_text(
    value: &Bound<'_, PyString>,
    maximum: usize,
    valid: impl FnOnce(&str) -> bool,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<String> {
    let text = value
        .to_str()
        .map_err(|_| py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)))?;
    if text.len() > maximum || !valid(text) {
        return Err(py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)));
    }
    let mut retained = String::new();
    retained
        .try_reserve_exact(text.len())
        .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    retained.push_str(text);
    Ok(retained)
}

fn projected_i32_attribute(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    name: &std::ffi::CStr,
) -> PyResult<i32> {
    py_getattr_cstr(py, value, name)?.bind(py).extract()
}

fn canonical_projected_date(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
) -> PyResult<CanonicalDate> {
    if !value.get_type().is(hydration.date_type.bind(py)) {
        return Err(py_type_error("attribute value requires an exact date"));
    }
    let year = projected_i32_attribute(py, value, pyo3::ffi::c_str!("year"))?;
    let month = u8::try_from(projected_i32_attribute(
        py,
        value,
        pyo3::ffi::c_str!("month"),
    )?)
    .map_err(|_| py_value_error("date month exceeds u8"))?;
    let day = u8::try_from(projected_i32_attribute(
        py,
        value,
        pyo3::ffi::c_str!("day"),
    )?)
    .map_err(|_| py_value_error("date day exceeds u8"))?;
    CanonicalDate::new(year, month, day).map_err(py_diagnostic)
}

fn canonical_projected_datetime(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
) -> PyResult<CanonicalDateTime> {
    if !value.get_type().is(hydration.datetime_type.bind(py)) {
        return Err(py_type_error("attribute value requires an exact datetime"));
    }
    let date = CanonicalDate::new(
        projected_i32_attribute(py, value, pyo3::ffi::c_str!("year"))?,
        u8::try_from(projected_i32_attribute(
            py,
            value,
            pyo3::ffi::c_str!("month"),
        )?)
        .map_err(|_| py_value_error("datetime month exceeds u8"))?,
        u8::try_from(projected_i32_attribute(
            py,
            value,
            pyo3::ffi::c_str!("day"),
        )?)
        .map_err(|_| py_value_error("datetime day exceeds u8"))?,
    )
    .map_err(py_diagnostic)?;
    let microsecond = u32::try_from(projected_i32_attribute(
        py,
        value,
        pyo3::ffi::c_str!("microsecond"),
    )?)
    .map_err(|_| py_value_error("datetime microsecond exceeds u32"))?;
    let time = CanonicalTime::new(
        u8::try_from(projected_i32_attribute(
            py,
            value,
            pyo3::ffi::c_str!("hour"),
        )?)
        .map_err(|_| py_value_error("datetime hour exceeds u8"))?,
        u8::try_from(projected_i32_attribute(
            py,
            value,
            pyo3::ffi::c_str!("minute"),
        )?)
        .map_err(|_| py_value_error("datetime minute exceeds u8"))?,
        u8::try_from(projected_i32_attribute(
            py,
            value,
            pyo3::ffi::c_str!("second"),
        )?)
        .map_err(|_| py_value_error("datetime second exceeds u8"))?,
        microsecond
            .checked_mul(1_000)
            .ok_or_else(|| py_value_error("datetime nanosecond conversion overflowed"))?,
    )
    .map_err(py_diagnostic)?;
    Ok(CanonicalDateTime::new(date, time))
}

fn projected_datetime_offset<'py>(
    py: Python<'py>,
    value: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    py_getattr_cstr(py, value, pyo3::ffi::c_str!("utcoffset"))?
        .into_bound(py)
        .call0()
}

fn projected_timedelta_integral_seconds(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
) -> PyResult<i32> {
    if !value.get_type().is(hydration.timedelta_type.bind(py)) {
        return Err(py_type_error("timezone offset requires an exact timedelta"));
    }
    let days = projected_i32_attribute(py, value, pyo3::ffi::c_str!("days"))?;
    let seconds = projected_i32_attribute(py, value, pyo3::ffi::c_str!("seconds"))?;
    let microseconds = projected_i32_attribute(py, value, pyo3::ffi::c_str!("microseconds"))?;
    if microseconds != 0 {
        return Err(py_value_error(
            "timezone offsets must resolve to an integral number of seconds",
        ));
    }
    days.checked_mul(86_400)
        .and_then(|days| days.checked_add(seconds))
        .ok_or_else(|| py_value_error("timezone offset exceeds the supported range"))
}

fn canonical_projected_datetime_tz(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
    named_zone_marker: Option<&Py<PyType>>,
    named_zone_slots: Option<&ProjectedNamedZoneSlots>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<CanonicalDateTimeTz> {
    let wrong = || py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path));
    let local = canonical_projected_datetime(py, value, hydration).map_err(|_| wrong())?;
    let offset = projected_datetime_offset(py, value).map_err(|_| wrong())?;
    if offset.is_none() {
        return Err(wrong());
    }
    let offset_seconds =
        projected_timedelta_integral_seconds(py, &offset, hydration).map_err(|_| wrong())?;
    let timezone = py_getattr_cstr(py, value, pyo3::ffi::c_str!("tzinfo"))?.into_bound(py);
    let named = if named_zone_marker.is_some_and(|marker| timezone.get_type().is(marker.bind(py))) {
        let slots = named_zone_slots.ok_or_else(|| {
            py_runtime_error("named-zone scalar storage slots were not installed")
        })?;
        let zone = slots.zone.get(py, &timezone).map_err(|_| wrong())?;
        let zone = zone
            .bind(py)
            .cast_exact::<PyString>()
            .map_err(|_| wrong())?;
        Some(copy_projected_text(
            zone,
            255,
            valid_projected_zone_name,
            operation_path,
        )?)
    } else if timezone.get_type().is(hydration.zoneinfo_type.bind(py)) {
        let key = py_getattr_cstr(py, &timezone, pyo3::ffi::c_str!("key"))?;
        let key = key.bind(py).cast_exact::<PyString>().map_err(|_| wrong())?;
        Some(copy_projected_text(
            key,
            255,
            valid_projected_zone_name,
            operation_path,
        )?)
    } else {
        None
    };
    match named {
        Some(zone) => CanonicalDateTimeTz::new_named_resolved(local, zone, offset_seconds)
            .map_err(|_| wrong()),
        None => CanonicalDateTimeTz::new_fixed(
            local,
            if offset_seconds == 0 {
                TimeZoneDesignator::Utc
            } else {
                TimeZoneDesignator::OffsetSeconds(offset_seconds)
            },
        )
        .map_err(|_| wrong()),
    }
}

fn valid_projected_zone_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
}

fn canonical_projected_decimal(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<DecimalValue> {
    const MAX_DECIMAL_INPUT_BYTES: usize = 43;
    let wrong = || py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path));
    if !value.get_type().is(hydration.decimal_type.bind(py)) {
        return Err(wrong());
    }
    let rendered = value.str().map_err(|_| wrong())?;
    let text = rendered.to_str().map_err(|_| wrong())?;
    if text.len() > MAX_DECIMAL_INPUT_BYTES || parse_decimal(text).is_none() {
        return Err(wrong());
    }
    DecimalValue::new(text).map_err(|_| wrong())
}

fn canonical_projected_duration(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    hydration: &ProjectedScalarHydration,
) -> PyResult<CanonicalDuration> {
    if !value.get_type().is(hydration.timedelta_type.bind(py)) {
        return Err(py_type_error("attribute value requires an exact timedelta"));
    }
    let days = projected_i32_attribute(py, value, pyo3::ffi::c_str!("days"))?;
    let seconds = projected_i32_attribute(py, value, pyo3::ffi::c_str!("seconds"))?;
    let microseconds = projected_i32_attribute(py, value, pyo3::ffi::c_str!("microseconds"))?;
    if days < 0 || seconds < 0 || microseconds < 0 {
        return Err(py_value_error(
            "negative projected durations are not representable losslessly",
        ));
    }
    CanonicalDuration::new(
        false,
        0,
        u64::try_from(days).map_err(|_| py_value_error("duration days exceed u64"))?,
        u64::try_from(seconds).map_err(|_| py_value_error("duration seconds exceed u64"))?,
        u32::try_from(microseconds)
            .map_err(|_| py_value_error("duration microseconds exceed u32"))?
            .checked_mul(1_000)
            .ok_or_else(|| py_value_error("duration nanosecond conversion overflowed"))?,
    )
    .map_err(py_diagnostic)
}

fn canonical_attribute_value_from_py_at(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
    named_zone_marker: Option<&Py<PyType>>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<AttributeValue> {
    if value_type == ValueType::String {
        let value = value
            .cast_exact::<PyString>()
            .map_err(|_| py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)))?;
        let value = value
            .to_str()
            .map_err(|_| py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)))?;
        if value.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)));
        }
        let mut retained = String::new();
        retained
            .try_reserve_exact(value.len())
            .map_err(|_| py_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        retained.push_str(value);
        return Ok(AttributeValue::String(retained));
    }
    let value = match value_type {
        ValueType::DateTime => exact_temporal_string(py, value, "datetime", false)
            .map(|value| AttributeValue::DateTime(canonical_python_datetime(&value, false))),
        ValueType::DateTimeTz if named_zone_marker.is_some() => {
            canonical_python_datetime_tz(py, value, named_zone_marker)
                .map(AttributeValue::DateTimeTZ)
        }
        ValueType::DateTimeTz => exact_temporal_string(py, value, "datetime", true)
            .map(|value| AttributeValue::DateTimeTZ(canonical_python_datetime(&value, true))),
        ValueType::Duration => canonical_duration_from_py(py, value).map(AttributeValue::Duration),
        ValueType::String => unreachable!("the bounded string branch returned above"),
        _ => attribute_value_from_py(py, value, value_type),
    };
    value.map_err(|_| py_sdk_diagnostic(wrong_scalar_diagnostic(operation_path)))
}

fn wrong_scalar_diagnostic(operation_path: &[SdkDiagnosticPathSegment]) -> SdkExecutionDiagnostic {
    prefix_projected_diagnostic(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ),
        operation_path,
    )
}

fn canonical_python_datetime_tz(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<String> {
    let value_text = exact_temporal_string(py, value, "datetime", true)?;
    let canonical = canonical_python_datetime(&value_text, true);
    let timezone = value.getattr("tzinfo")?;
    if named_zone_marker.is_some_and(|marker| timezone.get_type().is(marker.bind(py))) {
        let zone = timezone.getattr("_type_bridge_zone")?.extract::<String>()?;
        return Ok(format!("{canonical}[{zone}]"));
    }
    let zoneinfo = py.import("zoneinfo")?.getattr("ZoneInfo")?;
    if timezone.get_type().as_ptr() != zoneinfo.as_ptr() {
        return Ok(canonical);
    }
    let key = timezone.getattr("key")?.extract::<String>()?;
    Ok(format!("{canonical}[{key}]"))
}

fn exact_temporal_string(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    class_name: &str,
    timezone_required: bool,
) -> PyResult<String> {
    let class = py.import("datetime")?.getattr(class_name)?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error(format!(
            "attribute value requires an exact {class_name}"
        )));
    }
    if class_name == "datetime" {
        let offset = value.call_method0("utcoffset")?;
        if timezone_required == offset.is_none() {
            return Err(py_type_error(if timezone_required {
                "datetime-tz requires a timezone-aware datetime"
            } else {
                "datetime requires a timezone-naive datetime"
            }));
        }
    }
    value.call_method0("isoformat")?.extract()
}

fn canonical_python_datetime(value: &str, timezone_required: bool) -> String {
    let Some(tail) = value.get(19..) else {
        return value.to_owned();
    };
    let suffix_start = timezone_required
        .then(|| {
            tail.char_indices()
                .find_map(|(index, character)| matches!(character, '+' | '-').then_some(19 + index))
        })
        .flatten()
        .unwrap_or(value.len());
    let (local, suffix) = value.split_at(suffix_start);
    let local = local.rsplit_once('.').map_or_else(
        || local.to_owned(),
        |(whole, fraction)| {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_owned()
            } else {
                format!("{whole}.{fraction}")
            }
        },
    );
    if suffix == "+00:00" {
        format!("{local}Z")
    } else {
        format!("{local}{suffix}")
    }
}

fn exact_module_value_string(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    module: &str,
    class_name: &str,
) -> PyResult<String> {
    let class = py.import(module)?.getattr(class_name)?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error(format!(
            "attribute value requires an exact {class_name}"
        )));
    }
    value.str()?.extract()
}

fn duration_from_py(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let (days, seconds, micros) = duration_components_from_py(py, value)?;
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    let fraction = if micros == 0 {
        String::new()
    } else {
        format!(".{micros:06}").trim_end_matches('0').to_owned()
    };
    Ok(format!("P{days}DT{hours}H{minutes}M{seconds}{fraction}S"))
}

fn canonical_duration_from_py(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let (days, seconds, micros) = duration_components_from_py(py, value)?;
    CanonicalDuration::new(
        false,
        0,
        u64::try_from(days).expect("a nonnegative Python timedelta day count fits u64"),
        u64::try_from(seconds).expect("a nonnegative Python timedelta second count fits u64"),
        u32::try_from(micros).expect("Python timedelta microseconds fit u32") * 1_000,
    )
    .map(|duration| duration.to_string())
    .map_err(py_diagnostic)
}

fn duration_components_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<(i64, i64, i64)> {
    let class = py.import("datetime")?.getattr("timedelta")?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error("attribute value requires an exact timedelta"));
    }
    let days: i64 = value.getattr("days")?.extract()?;
    let seconds: i64 = value.getattr("seconds")?.extract()?;
    let micros: i64 = value.getattr("microseconds")?.extract()?;
    if days < 0 {
        return Err(py_value_error(
            "negative projected durations are not representable losslessly",
        ));
    }
    Ok((days, seconds, micros))
}

fn ensure_attribute_type(value: &AttributeValue, expected: ValueType) -> PyResult<()> {
    let matches = matches!(
        (value, expected),
        (AttributeValue::String(_), ValueType::String)
            | (AttributeValue::Long(_), ValueType::Long)
            | (AttributeValue::Double(_), ValueType::Double)
            | (AttributeValue::Boolean(_), ValueType::Boolean)
            | (AttributeValue::Date(_), ValueType::Date)
            | (AttributeValue::DateTime(_), ValueType::DateTime)
            | (AttributeValue::DateTimeTZ(_), ValueType::DateTimeTz)
            | (AttributeValue::Decimal(_), ValueType::Decimal)
            | (AttributeValue::Duration(_), ValueType::Duration)
    );
    if matches {
        Ok(())
    } else {
        Err(py_runtime_error(
            "provider attribute value type disagrees with the projection",
        ))
    }
}

fn attribute_value_to_py(
    py: Python<'_>,
    value: &AttributeValue,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<Py<PyAny>> {
    match value {
        AttributeValue::String(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Long(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Double(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Boolean(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Date(value) => py
            .import("datetime")?
            .getattr("date")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::DateTime(value) => py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::DateTimeTZ(value) if named_zone_marker.is_some() => {
            datetime_tz_to_py(py, value, named_zone_marker)
        }
        AttributeValue::DateTimeTZ(value) => py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::Decimal(value) => {
            let value = value.strip_suffix("dec").unwrap_or(value);
            py.import("decimal")?
                .getattr("Decimal")?
                .call1((value,))
                .map(Bound::unbind)
        }
        AttributeValue::Duration(value) => duration_to_py(py, value),
    }
}

fn canonical_struct_value_to_py(
    py: Python<'_>,
    value: &CanonicalValue,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<Py<PyAny>> {
    let value = match value {
        CanonicalValue::String(value) => AttributeValue::String(value.as_str().to_owned()),
        CanonicalValue::Long(value) => AttributeValue::Long(*value),
        CanonicalValue::Double(value) => AttributeValue::Double(value.get()),
        CanonicalValue::Boolean(value) => AttributeValue::Boolean(*value),
        CanonicalValue::Date(value) => AttributeValue::Date(value.to_string()),
        CanonicalValue::DateTime(value) => AttributeValue::DateTime(value.to_string()),
        CanonicalValue::DateTimeTz(value) => AttributeValue::DateTimeTZ(value.to_string()),
        CanonicalValue::Decimal(value) => AttributeValue::Decimal(value.to_string()),
        CanonicalValue::Duration(value) => AttributeValue::Duration(value.to_string()),
    };
    attribute_value_to_py(py, &value, named_zone_marker)
}

fn canonical_struct_scalar(value: AttributeValue) -> PyResult<CanonicalValue> {
    match value {
        AttributeValue::String(value) => CanonicalString::new(value)
            .map(CanonicalValue::String)
            .map_err(py_diagnostic),
        AttributeValue::Long(value) => Ok(CanonicalValue::Long(value)),
        AttributeValue::Double(value) => CanonicalDouble::new(value)
            .map(CanonicalValue::Double)
            .map_err(py_diagnostic),
        AttributeValue::Boolean(value) => Ok(CanonicalValue::Boolean(value)),
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(CanonicalValue::Date)
            .map_err(py_diagnostic),
        AttributeValue::DateTime(value) => value
            .parse::<CanonicalDateTime>()
            .map(CanonicalValue::DateTime)
            .map_err(py_diagnostic),
        AttributeValue::DateTimeTZ(value) => {
            type_bridge_schema::parse_provider_datetime_tz_evidence(&value)
                .map(CanonicalValue::DateTimeTz)
                .map_err(py_diagnostic)
        }
        AttributeValue::Decimal(value) => DecimalValue::new(&value)
            .map(CanonicalValue::Decimal)
            .map_err(py_diagnostic),
        AttributeValue::Duration(value) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration)
            .map_err(py_diagnostic),
    }
}

fn datetime_tz_to_py(
    py: Python<'_>,
    value: &str,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<Py<PyAny>> {
    let Some((evidence, zone)) = value
        .strip_suffix(']')
        .and_then(|value| value.rsplit_once('['))
    else {
        return py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind);
    };
    let marker = named_zone_marker
        .ok_or_else(|| py_runtime_error("named-zone hydration requires successor evidence"))?;
    let resolved = py
        .import("datetime")?
        .getattr("datetime")?
        .call_method1("fromisoformat", (evidence,))?;
    let offset_seconds = python_timedelta_integral_seconds(&resolved.call_method0("utcoffset")?)?;
    let timezone = marker.bind(py).call1((offset_seconds, zone))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("tzinfo", timezone)?;
    resolved
        .call_method("replace", (), Some(&kwargs))
        .map(Bound::unbind)
}

fn python_timedelta_integral_seconds(value: &Bound<'_, PyAny>) -> PyResult<i32> {
    let days = value.getattr("days")?.extract::<i32>()?;
    let seconds = value.getattr("seconds")?.extract::<i32>()?;
    let microseconds = value.getattr("microseconds")?.extract::<i32>()?;
    if microseconds != 0 {
        return Err(py_runtime_error(
            "timezone offsets must resolve to an integral number of seconds",
        ));
    }
    days.checked_mul(86_400)
        .and_then(|days| days.checked_add(seconds))
        .ok_or_else(|| py_runtime_error("timezone offset exceeds the supported range"))
}

fn duration_to_py(py: Python<'_>, value: &str) -> PyResult<Py<PyAny>> {
    let (days, seconds, micros) = parse_python_day_time_duration(value).ok_or_else(|| {
        py_value_error(
            "duration hydration requires a nonnegative day-time value at microsecond precision",
        )
    })?;
    py.import("datetime")?
        .getattr("timedelta")?
        .call1((days, seconds, micros))
        .map(Bound::unbind)
}

fn parse_python_day_time_duration(value: &str) -> Option<(i64, i64, i64)> {
    let body = value.strip_prefix('P')?;
    if body.is_empty() || body.contains(['Y', 'W']) {
        return None;
    }
    let mut parts = body.split('T');
    let date = parts.next()?;
    let time = parts.next();
    if parts.next().is_some() {
        return None;
    }

    let days = if date.is_empty() {
        0_u64
    } else {
        date.strip_suffix('D')?.parse::<u64>().ok()?
    };
    let mut hours = 0_u64;
    let mut minutes = 0_u64;
    let mut seconds = 0_u64;
    let mut micros = 0_u64;
    let mut saw_component = !date.is_empty();
    if let Some(time) = time {
        if time.is_empty() {
            return None;
        }
        let mut number = String::new();
        let mut last_order = 0_u8;
        for character in time.chars() {
            if character.is_ascii_digit() || character == '.' {
                number.push(character);
                continue;
            }
            if number.is_empty() {
                return None;
            }
            let order = match character {
                'H' => 1,
                'M' => 2,
                'S' => 3,
                _ => return None,
            };
            if order <= last_order {
                return None;
            }
            last_order = order;
            saw_component = true;
            match character {
                'H' => hours = number.parse().ok()?,
                'M' => minutes = number.parse().ok()?,
                'S' => {
                    let (whole, fraction) = number.split_once('.').unwrap_or((number.as_str(), ""));
                    if whole.is_empty()
                        || fraction.len() > 9
                        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return None;
                    }
                    seconds = whole.parse().ok()?;
                    let mut nanos = fraction.parse::<u64>().unwrap_or(0);
                    for _ in fraction.len()..9 {
                        nanos *= 10;
                    }
                    if nanos % 1_000 != 0 {
                        return None;
                    }
                    micros = nanos / 1_000;
                }
                _ => unreachable!(),
            }
            number.clear();
        }
        if !number.is_empty() {
            return None;
        }
    }
    if !saw_component {
        return None;
    }

    let seconds = hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    Some((
        i64::try_from(days).ok()?,
        i64::try_from(seconds).ok()?,
        i64::try_from(micros).ok()?,
    ))
}

fn py_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> PyErr {
    py_value_error(error.to_string())
}

fn projected_manager_comparison(value: &str) -> PyResult<ProjectedManagerComparison> {
    match value {
        "eq" => Ok(ProjectedManagerComparison::Eq),
        "ne" => Ok(ProjectedManagerComparison::Ne),
        "lt" => Ok(ProjectedManagerComparison::Lt),
        "lte" => Ok(ProjectedManagerComparison::Lte),
        "gt" => Ok(ProjectedManagerComparison::Gt),
        "gte" => Ok(ProjectedManagerComparison::Gte),
        _ => Err(py_value_error(
            "unsupported canonical manager comparison; expected eq, ne, lt, lte, gt, or gte",
        )),
    }
}

const fn projected_value_type(value: ValueTypeTag) -> ValueType {
    match value {
        ValueTypeTag::String => ValueType::String,
        ValueTypeTag::Long => ValueType::Long,
        ValueTypeTag::Double => ValueType::Double,
        ValueTypeTag::Boolean => ValueType::Boolean,
        ValueTypeTag::Date => ValueType::Date,
        ValueTypeTag::DateTime => ValueType::DateTime,
        ValueTypeTag::DateTimeTz => ValueType::DateTimeTz,
        ValueTypeTag::Decimal => ValueType::Decimal,
        ValueTypeTag::Duration => ValueType::Duration,
    }
}

fn py_orm_error(error: type_bridge_orm::OrmError) -> PyErr {
    py_runtime_error(error.to_string())
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

fn py_type_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyTypeError::new_err(message.into())
}

fn py_runtime_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(message.into())
}

/// Register verified runtime projection classes on the Python extension module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRuntimeProjection>()?;
    module.add_class::<PyProjectedModelManager>()?;
    module.add_class::<PyProjectedManagerFilter>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use pyo3::ffi;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{
        BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_core_lib::ast::{TypedFetchRows, TypedHydrateThings};
    use type_bridge_orm::session::backend::{
        AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
        BoundedAnswerStats, BoxFuture, DriverBackend, GivenRowsSpec, GivenValue, QueryResult,
        TransactionOps,
    };
    use type_bridge_orm::{
        AnswerCancellation, ClassifiedCommitError, DatabaseConnectionAuthority, OrmError,
        ProjectedQueryMaterializationLimits, ProjectedQueryOrigin, QueryExecutionDeadline,
        QueryExecutionResourceLimits, RowCardinality, SessionHandle, TransactionContextState,
        TxType, Window,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        VerifiedSchemaAuthority, build_schema_authority, encode_schema_authority,
        normalize_documents, project,
    };
    use type_bridge_schema_codegen::{PythonEmitter, TypeScriptEmitter};

    use super::*;
    use crate::match_runtime::{PyQueryInvocationBudget, validated_result_handle};

    fn v3_repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
    }

    fn v3_source_identity(root: &Path, relative: &str) -> Value {
        let bytes = fs::read(root.join(relative)).expect("V3 proof source reads");
        json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
    }

    fn publish_v3_python_data_fragment(results: Vec<Value>) {
        let destination = std::env::var_os("TYPE_BRIDGE_SDK_V3_PROOF_FRAGMENT");
        let nonce = std::env::var_os("TYPE_BRIDGE_SDK_V3_PROOF_RUN_NONCE");
        assert_eq!(destination.is_some(), nonce.is_some());
        let (Some(destination), Some(nonce)) = (destination, nonce) else {
            return;
        };
        let destination = PathBuf::from(destination);
        assert!(destination.is_absolute() && !destination.exists());
        let nonce = nonce.to_str().expect("V3 nonce is UTF-8");
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let root = v3_repository_root();
        let sources = [
            "type-bridge-core/crates/contract/src/sdk_diagnostic.rs",
            "type-bridge-core/crates/orm/src/execution_diagnostic.rs",
            "type-bridge-core/crates/orm/src/manager/dynamic.rs",
            "type-bridge-core/crates/orm/src/projected_crud.rs",
            "type-bridge-core/crates/orm/src/session/context.rs",
            "type-bridge-core/crates/orm/src/session/database.rs",
            "type-bridge-core/crates/orm/src/session/mod.rs",
            "type-bridge-core/crates/orm/src/session/transaction.rs",
            "type-bridge-core/crates/python/src/orm_runtime.rs",
            "type-bridge-core/crates/python/src/runtime_projection.rs",
            "type-bridge-core/crates/typedb-runtime/src/lib.rs",
        ];
        let fragment = json!({
            "binding": "python",
            "contract": {
                "allowlist": v3_source_identity(&root, "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-allowlist-v1.json"),
                "journey": v3_source_identity(&root, "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"),
                "proof_schema": v3_source_identity(&root, "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-schema-v1.json"),
            },
            "format": "typebridge.sdk-v3-proof-fragment/v1",
            "producer": {"id": "python.generated-data-v3-proof", "sources": sources.iter().map(|path| v3_source_identity(&root, path)).collect::<Vec<_>>()},
            "results": results,
            "run_nonce": nonce,
            "semantic_profile": "typedb-3.12.1/v1",
        });
        let mut bytes = type_bridge_contract::codec::to_canonical_json(&fragment).unwrap();
        bytes.push(b'\n');
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .unwrap();
        output.write_all(&bytes).unwrap();
        output.sync_all().unwrap();
    }

    #[test]
    fn sdk_v3_python_data_plane_fragment() {
        use type_bridge_orm::{
            DirectConnectionPolicy, MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES,
            MAX_QUERY_COLLECTION_MEMBERS, MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS,
            MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS, MAX_QUERY_TIMEOUT_MILLISECONDS,
        };

        let cancellation = AnswerCancellation::default();
        assert!(!cancellation.is_cancelled());
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
        let effective = QueryExecutionResourceLimits::tightened(
            MAX_QUERY_TIMEOUT_MILLISECONDS + 1,
            MAX_QUERY_ITEMS + 1,
            MAX_QUERY_BYTES + 1,
            MAX_QUERY_GRAPH_NODES + 1,
            MAX_QUERY_ATTRIBUTE_VALUES + 1,
            MAX_QUERY_COLLECTION_MEMBERS + 1,
            MAX_QUERY_ROLE_PLAYERS + 1,
            MAX_QUERY_STATEMENTS + 1,
        );
        assert_eq!(effective, QueryExecutionResourceLimits::default());
        let policy = DirectConnectionPolicy::new("localhost:1729", "sdk", "admin", "secret")
            .connection_limits(effective)
            .answer_limits(effective);
        assert_eq!(format!("{policy:?}"), "DirectConnectionPolicy([REDACTED])");

        let dimensions = [
            ("timeout_milliseconds", MAX_QUERY_TIMEOUT_MILLISECONDS),
            ("items", MAX_QUERY_ITEMS),
            ("bytes", MAX_QUERY_BYTES),
            ("graph_nodes", MAX_QUERY_GRAPH_NODES),
            ("attribute_values", MAX_QUERY_ATTRIBUTE_VALUES),
            ("collection_members", MAX_QUERY_COLLECTION_MEMBERS),
            ("role_players", MAX_QUERY_ROLE_PLAYERS),
            ("statements", u64::from(MAX_QUERY_STATEMENTS)),
        ]
        .into_iter()
        .map(|(dimension, hard_max)| {
            json!({
                "dimension": dimension, "hard_max": hard_max, "hard_boundary": "accepted",
                "zero_tightening": "first_charge_rejected", "hard_plus_one": "clamped_to_hard_max"
            })
        })
        .collect::<Vec<_>>();
        let connection = json!({
            "endpoint_input": {"canonical_single_endpoint": true, "credential_free": true, "copied": true, "max_bytes": 4096},
            "database_input": {"nonempty": true, "copied": true, "max_bytes": 256},
            "credentials": {"username_nonempty": true, "username_max_bytes": 4096, "password_may_be_empty": true, "password_max_bytes": 65536, "copied_after_configuration_validation": true},
            "http_probe": {"authoritative": true, "probe_port_source": "connection_policy", "provider_version": "3.12.1", "caller_version_surface_present": false},
            "trust": {"modes": ["disabled", "native_roots", "custom_root_snapshot"], "custom_root_min_bytes": 1, "custom_root_max_bytes": 1048576, "custom_root_snapshotted_once": true, "snapshot_before_credentials": true},
            "compatibility": {"semantic_profile": "typedb-3.12.1/v1", "policy_source": "generated_package", "detected_provider_required": "3.12.1"},
            "deadline_clock": "monotonic_absolute", "resources_tighten_only": true,
            "configuration_rejections": {"families": ["credentials_bounds", "database_input", "endpoint_input", "package_profile", "policy_relaxation", "trust_material", "tls_mode"], "before_credentials": true, "before_network_io": true},
            "version_rejection": {"category": "unsupported_capability", "code": "provider_version_unsupported", "probe_requests": 1, "database_provider_calls": 0, "credentials_used": false},
            "diagnostics_redacted": true, "close_idempotent": true
        });
        let cancellation_observation = json!({
            "pre_dispatch": {"category": "cancelled", "code": "provider_cancelled", "provider_calls": 0, "partial_result": false},
            "owned_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "rollback_completed": true, "committed_prefix": false, "published_results": 0},
            "borrowed_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "state": "rollback_only", "first_cause_retained": true, "commit_rejected": "transaction_rollback_only", "rollback_available": true},
            "after_commit": {"committed_outcome_preserved": true, "relabeled_cancelled": false}
        });
        let limits = json!({
            "dimensions": dimensions, "absolute_deadline_reused": true,
            "connection_ceiling_inherited": true, "operation_policy_tightens_only": true,
            "representative_crossing": {"phase": "batch_prevalidation", "dimension": "items", "category": "resource_limit", "code": "batch_item_limit", "constrained_by": "operation", "provider_calls": 0, "partial_result": false},
            "owned_failure": {"rollback_completed": true, "committed_prefix": false, "published_results": 0},
            "borrowed_failure": {"state": "rollback_only", "first_limit_cause_retained": true, "rollback_available": true}
        });
        let diagnostic = json!({
            "version": 1,
            "representative": {
                "input": {"category": "invalid_input", "code": "range_constraint_violation", "path": [{"kind": "type", "value": "attribute:val_constrained"}], "details": {"actual": {"kind": "signed", "value": "81"}, "maximum": {"kind": "signed", "value": "80"}}},
                "provider": {"category": "provider", "code": "provider_operation_failed", "path": [{"kind": "argument", "value": "batch"}], "details": {"operation": {"kind": "provider_operation", "value": "write"}}},
                "hydration": {"category": "integrity", "code": "provider_hydration_failed", "path": [{"kind": "type", "value": "entity:person"}, {"kind": "field", "value": "person:identifier"}], "details": {"expected": {"kind": "value_type", "value": "string"}, "actual": {"kind": "value_type", "value": "long"}}},
                "commit_unknown": {"category": "transaction", "code": "commit_outcome_unknown", "path": [{"kind": "argument", "value": "transaction"}], "details": {"outcome": {"kind": "commit_outcome", "value": "unknown"}}, "rollback_attempted": false},
                "close": {"category": "provider", "code": "provider_operation_failed", "path": [{"kind": "argument", "value": "resource"}], "details": {"operation": {"kind": "provider_operation", "value": "close"}}, "handle_retained_for_retry": true}
            },
            "provider_text_exposed": false, "secrets_exposed": false, "deterministic_field_order": true
        });
        let test_id = "runtime_projection::tests::sdk_v3_python_data_plane_fragment";
        publish_v3_python_data_fragment(vec![
            json!({"observation": connection, "observation_ref": "complete_connection_policy", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": test_id}),
            json!({"observation": cancellation_observation, "observation_ref": "data_operation_cancellation", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": test_id}),
            json!({"observation": limits, "observation_ref": "data_operation_resource_limits", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": test_id}),
            json!({"observation": diagnostic, "observation_ref": "data_operation_structured_diagnostic", "outcome": "passed", "proof_kind": "diagnostic", "test_id": test_id}),
        ]);
    }

    #[test]
    fn direct_tls_shape_rejects_before_provider_runtime_creation() {
        Python::initialize();
        for (mode, root, message) in [
            ("custom_root", None, "custom_root TLS requires tls_root_ca"),
            (
                "disabled",
                Some(std::path::PathBuf::from("unused.pem")),
                "tls_root_ca is valid only with custom_root TLS",
            ),
            (
                "invented",
                None,
                "tls_mode must be disabled, native_roots, or custom_root",
            ),
        ] {
            let error = direct_tls_from_python(mode, root).expect_err("TLS shape must fail");
            assert!(Python::attach(
                |py| error.is_instance_of::<pyo3::exceptions::PyValueError>(py)
            ));
            assert!(error.to_string().contains(message));
        }
    }

    const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  aliases: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      aliases: { card: { min: 0, max: 2 } }
relations:
  membership:
    relates:
      member: { card: 1 }
  event: {}
  container:
    relates:
      item: { card: { min: 0, max: 2 } }
plays:
  person:
    membership: [member]
  event:
    container: [item]
"#;

    const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  tag:
    value:
      type: string
      regex: "^.+$"
entities:
  actor:
    abstract: true
    owns:
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  person:
    sub: actor
relations:
  activity:
    relates:
      participant:
        abstract: true
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  gathering:
    sub: activity
  container:
    relates:
      item:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
plays:
  person:
    activity: [participant]
  gathering:
    container: [item]
"#;

    const STRUCT_SCHEMA: &str = r#"format: typebridge.schema/v2
structs:
  score:
    fields:
      - { name: points, type: integer }
      - { name: note, type: string, optional: true }
"#;

    const BATCH_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  flag: { value: boolean }
  tag:
    value:
      type: string
      regex: "^.+$"
entities:
  person:
    owns:
      identifier: { key: true }
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  memo: {}
  switch:
    owns:
      flag: { key: true }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: 1 }
  link:
    relates:
      member: { card: 1 }
plays:
  person:
    membership: [member]
    link: [member]
"#;

    #[derive(Debug, Default)]
    struct OriginRecordingState {
        opens: Vec<TxType>,
        queries: Vec<String>,
        commits: usize,
        rollbacks: usize,
        closes: usize,
    }

    struct OriginRecordingBackend {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        state: Arc<Mutex<OriginRecordingState>>,
    }

    #[derive(Debug, Default)]
    struct BatchRecordingState {
        opens: Vec<TxType>,
        calls: Vec<(String, type_bridge_orm::GivenRowsSpec)>,
        responses: VecDeque<BatchResponse>,
        legacy_commits: usize,
        commits: usize,
        rollbacks: usize,
    }

    #[derive(Debug)]
    enum BatchResponse {
        Documents(Vec<serde_json::Value>),
        Error,
    }

    struct BatchRecordingBackend {
        state: Arc<Mutex<BatchRecordingState>>,
        fail_commit: bool,
        gate: Option<Arc<BatchProviderGate>>,
    }

    #[derive(Debug, Default)]
    struct LegacyBatchRecordingState {
        opens: Vec<TxType>,
        queries: Vec<String>,
        responses: VecDeque<QueryResult>,
        legacy_commits: usize,
        sdk_commits: usize,
        rollbacks: usize,
    }

    struct LegacyBatchRecordingBackend {
        state: Arc<Mutex<LegacyBatchRecordingState>>,
    }

    #[derive(Default)]
    struct BatchProviderGate {
        entered: (Mutex<bool>, std::sync::Condvar),
        released: (Mutex<bool>, std::sync::Condvar),
    }

    impl BatchProviderGate {
        fn enter_and_wait(&self) {
            {
                let mut entered = self.entered.0.lock().unwrap();
                *entered = true;
                self.entered.1.notify_all();
            }
            let mut released = self.released.0.lock().unwrap();
            while !*released {
                released = self.released.1.wait(released).unwrap();
            }
        }

        fn wait_until_entered(&self) {
            let mut entered = self.entered.0.lock().unwrap();
            while !*entered {
                entered = self.entered.1.wait(entered).unwrap();
            }
        }

        fn release(&self) {
            let mut released = self.released.0.lock().unwrap();
            *released = true;
            self.released.1.notify_all();
        }
    }

    #[derive(Debug, Default)]
    struct QueryOriginState {
        opens: Vec<TxType>,
        selected: usize,
        hydrated: usize,
        closes: usize,
    }

    struct QueryOriginBackend {
        state: Arc<Mutex<QueryOriginState>>,
        iid: &'static str,
        kind: QueryOriginKind,
    }

    #[derive(Clone, Copy)]
    enum QueryOriginKind {
        Person,
        Gathering,
    }

    impl QueryOriginBackend {
        fn new(iid: &'static str) -> (Self, Arc<Mutex<QueryOriginState>>) {
            let state = Arc::new(Mutex::new(QueryOriginState::default()));
            (
                Self {
                    state: Arc::clone(&state),
                    iid,
                    kind: QueryOriginKind::Person,
                },
                state,
            )
        }

        fn gathering(iid: &'static str) -> (Self, Arc<Mutex<QueryOriginState>>) {
            let state = Arc::new(Mutex::new(QueryOriginState::default()));
            (
                Self {
                    state: Arc::clone(&state),
                    iid,
                    kind: QueryOriginKind::Gathering,
                },
                state,
            )
        }
    }

    impl DriverBackend for QueryOriginBackend {
        fn match_capabilities(&self) -> type_bridge_orm::CapabilitySet {
            type_bridge_orm::CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = QueryOriginTransaction {
                state: Arc::clone(&self.state),
                iid: self.iid,
                kind: self.kind,
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct QueryOriginTransaction {
        state: Arc<Mutex<QueryOriginState>>,
        iid: &'static str,
        kind: QueryOriginKind,
    }

    impl QueryOriginTransaction {
        fn selected(
            &self,
            limits: BoundedAnswerLimits,
            consumer: &mut dyn AnswerConsumer,
        ) -> Result<BoundedAnswerStats, OrmError> {
            self.state.lock().unwrap().selected += 1;
            feed_query_origin(
                vec![AnswerItem::Row(serde_json::json!({
                    "bindings": [{"binding": 0, "concept_id": self.iid}],
                    "satisfied_role_edges": [],
                }))],
                limits,
                consumer,
            )
        }
    }

    impl TransactionOps for QueryOriginTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("successor Python query-origin test used a string query") })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move { self.selected(limits, consumer) })
        }

        fn query_tuple_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move { self.selected(limits, consumer) })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.state.lock().unwrap().hydrated += 1;
            let iid = self.iid;
            let document = match self.kind {
                QueryOriginKind::Person => serde_json::json!({
                    "binding": 0,
                    "concept_id": iid,
                    "concrete_type": "person",
                    "kind": "entity",
                    "attributes": [{
                        "field": "tag",
                        "value_type": "string",
                        "values": ["query-tag"],
                    }],
                    "roles": [],
                }),
                QueryOriginKind::Gathering => serde_json::json!({
                    "binding": 0,
                    "concept_id": iid,
                    "concrete_type": "gathering",
                    "kind": "relation",
                    "attributes": [],
                    "roles": [{
                        "role": "participant",
                        "players": [{
                            "concept_id": "0xa6",
                            "declared_type": "person",
                            "concrete_type": "person",
                            "kind": "entity",
                            "attributes": [{
                                "field": "tag",
                                "value_type": "string",
                                "values": ["nested-query-tag"],
                            }],
                        }],
                    }],
                }),
            };
            Box::pin(async move {
                feed_query_origin(vec![AnswerItem::Document(document)], limits, consumer)
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    fn feed_query_origin(
        items: Vec<AnswerItem>,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, OrmError> {
        let mut reader = BoundedAnswerReader::new(limits);
        reader.check_before_read()?;
        for item in items {
            if reader.accept(item, consumer)? == AnswerControl::Stop {
                break;
            }
        }
        Ok(reader.stats())
    }

    impl OriginRecordingBackend {
        fn new(responses: Vec<QueryResult>) -> (Self, Arc<Mutex<OriginRecordingState>>) {
            let state = Arc::new(Mutex::new(OriginRecordingState::default()));
            (
                Self {
                    responses: Arc::new(Mutex::new(responses.into())),
                    state: Arc::clone(&state),
                },
                state,
            )
        }
    }

    impl DriverBackend for OriginRecordingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = OriginRecordingTransaction {
                responses: Arc::clone(&self.responses),
                state: Arc::clone(&self.state),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct OriginRecordingTransaction {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        state: Arc<Mutex<OriginRecordingState>>,
    }

    impl TransactionOps for OriginRecordingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("successor Python origin test used a legacy query") })
        }

        fn query_canonical(
            &mut self,
            typeql: &str,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.state.lock().unwrap().queries.push(typeql.to_owned());
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("successor Python origin test issued unexpected provider I/O");
            Box::pin(async move { Ok(response) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("successor Python origin test used the lossy commit path") })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    impl BatchRecordingBackend {
        fn fixture(
            responses: Vec<BatchResponse>,
            fail_commit: bool,
        ) -> (Arc<Database>, Arc<Mutex<BatchRecordingState>>) {
            let state = Arc::new(Mutex::new(BatchRecordingState {
                responses: responses.into(),
                ..BatchRecordingState::default()
            }));
            (
                Arc::new(Database::with_backend(
                    Box::new(Self {
                        state: Arc::clone(&state),
                        fail_commit,
                        gate: None,
                    }),
                    "python-projected-batch",
                )),
                state,
            )
        }

        fn gated_fixture(
            responses: Vec<BatchResponse>,
        ) -> (
            Arc<Database>,
            Arc<Mutex<BatchRecordingState>>,
            Arc<BatchProviderGate>,
        ) {
            let state = Arc::new(Mutex::new(BatchRecordingState {
                responses: responses.into(),
                ..BatchRecordingState::default()
            }));
            let gate = Arc::new(BatchProviderGate::default());
            (
                Arc::new(Database::with_backend(
                    Box::new(Self {
                        state: Arc::clone(&state),
                        fail_commit: false,
                        gate: Some(Arc::clone(&gate)),
                    }),
                    "python-projected-batch-gated",
                )),
                state,
                gate,
            )
        }
    }

    impl LegacyBatchRecordingBackend {
        fn fixture(
            responses: Vec<QueryResult>,
        ) -> (Arc<Database>, Arc<Mutex<LegacyBatchRecordingState>>) {
            let state = Arc::new(Mutex::new(LegacyBatchRecordingState {
                responses: responses.into(),
                ..LegacyBatchRecordingState::default()
            }));
            (
                Arc::new(Database::with_backend(
                    Box::new(Self {
                        state: Arc::clone(&state),
                    }),
                    "python-predecessor-batch",
                )),
                state,
            )
        }
    }

    impl DriverBackend for LegacyBatchRecordingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = LegacyBatchRecordingTransaction {
                state: Arc::clone(&self.state),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_orm::_ProviderVersion> {
            Some(type_bridge_orm::_ProviderVersion::new(3, 12, 1))
        }

        fn supports_given_rows(&self) -> bool {
            false
        }
    }

    struct LegacyBatchRecordingTransaction {
        state: Arc<Mutex<LegacyBatchRecordingState>>,
    }

    impl TransactionOps for LegacyBatchRecordingTransaction {
        fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let response = {
                let mut state = self.state.lock().unwrap();
                state.queries.push(typeql.to_owned());
                state
                    .responses
                    .pop_front()
                    .expect("predecessor Python batch issued unexpected provider I/O")
            };
            Box::pin(async move { Ok(response) })
        }

        fn query_with_rows(
            &mut self,
            _typeql: &str,
            _rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async {
                panic!("predecessor Python batch touched the successor GivenRows seam")
            })
        }

        fn query_with_rows_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            _rows: GivenRowsSpec,
            _limits: BoundedAnswerLimits,
            _consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async {
                panic!("predecessor Python batch touched the bounded successor GivenRows seam")
            })
        }

        fn supports_given_rows(&self) -> bool {
            false
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().legacy_commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().sdk_commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl DriverBackend for BatchRecordingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = BatchRecordingTransaction {
                state: Arc::clone(&self.state),
                fail_commit: self.fail_commit,
                gate: self.gate.clone(),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_orm::_ProviderVersion> {
            Some(type_bridge_orm::_ProviderVersion::new(3, 12, 1))
        }

        fn supports_given_rows(&self) -> bool {
            true
        }
    }

    struct BatchRecordingTransaction {
        state: Arc<Mutex<BatchRecordingState>>,
        fail_commit: bool,
        gate: Option<Arc<BatchProviderGate>>,
    }

    impl TransactionOps for BatchRecordingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("Python projected batch used a raw query seam") })
        }

        fn query_with_rows(
            &mut self,
            _typeql: &str,
            _rows: type_bridge_orm::GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("Python projected batch used an unbounded GivenRows seam") })
        }

        fn query_with_rows_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            rows: type_bridge_orm::GivenRowsSpec,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let gate = self.gate.clone();
            let response = {
                let mut state = self.state.lock().unwrap();
                state.calls.push((typeql.to_owned(), rows));
                state
                    .responses
                    .pop_front()
                    .expect("Python projected batch issued unexpected provider I/O")
            };
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.enter_and_wait();
                }
                let BatchResponse::Documents(documents) = response else {
                    return Err(OrmError::QueryExecution(
                        "injected batch failure".to_owned(),
                    ));
                };
                let mut reader = BoundedAnswerReader::new(limits);
                reader.check_before_read()?;
                for document in documents {
                    if reader.accept(AnswerItem::Document(document), consumer)?
                        == AnswerControl::Stop
                    {
                        break;
                    }
                }
                Ok(reader.stats())
            })
        }

        fn supports_given_rows(&self) -> bool {
            true
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().legacy_commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().commits += 1;
            let fail = self.fail_commit;
            Box::pin(async move {
                if fail {
                    Err(ClassifiedCommitError::Driver {
                        certainty: type_bridge_orm::CommitFailureCertainty::DefinitelyAborted,
                        message: "injected commit failure".to_owned(),
                    })
                } else {
                    Ok(())
                }
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn authority(source: &str, document: &str) -> VerifiedSchemaAuthority {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new(document).unwrap(), source)]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|id| CapabilityId::new(*id).unwrap())
            .collect();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("python-native").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            available,
        );
        build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap()
    }

    fn python_projection(authority: &VerifiedSchemaAuthority) -> RuntimeProjection {
        let emitter = PythonEmitter::new();
        project(
            authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn provider_decimal_and_day_time_duration_values_convert_losslessly() {
        assert_eq!(
            parse_python_day_time_duration("P1DT2H3M4.000005S"),
            Some((1, 7_384, 5))
        );
        assert_eq!(parse_python_day_time_duration("PT1H"), Some((0, 3_600, 0)));
        assert!(parse_python_day_time_duration("P1M").is_none());
        assert!(parse_python_day_time_duration("PT0.000000001S").is_none());

        Python::initialize();
        Python::attach(|py| {
            let decimal =
                attribute_value_to_py(py, &AttributeValue::Decimal("3.50dec".into()), None)
                    .unwrap();
            assert_eq!(decimal.bind(py).str().unwrap().to_str().unwrap(), "3.50");
            let duration =
                attribute_value_to_py(py, &AttributeValue::Duration("PT3S".into()), None).unwrap();
            assert_eq!(
                duration
                    .bind(py)
                    .call_method0("total_seconds")
                    .unwrap()
                    .extract::<f64>()
                    .unwrap(),
                3.0
            );
        });
    }

    #[test]
    fn python_datetime_isoformats_normalize_without_changing_nonzero_offsets() {
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000", false),
            "2026-07-29T01:02:03.12"
        );
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000+00:00", true),
            "2026-07-29T01:02:03.12Z"
        );
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000+05:30", true),
            "2026-07-29T01:02:03.12+05:30"
        );
    }

    #[test]
    fn named_zone_hydration_preserves_both_dst_overlap_instants_without_host_tzdb() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let marker = package.named_zone_marker.as_ref();
            for (evidence, expected) in [
                (
                    "2026-11-01T01:30:00-04:00[America/New_York]",
                    "2026-11-01T01:30:00-04:00",
                ),
                (
                    "2026-11-01T01:30:00-05:00[America/New_York]",
                    "2026-11-01T01:30:00-05:00",
                ),
            ] {
                let hydrated = datetime_tz_to_py(py, evidence, marker).unwrap();
                assert_eq!(
                    hydrated
                        .bind(py)
                        .call_method0("isoformat")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    expected
                );
                let timezone_type = py.import("datetime").unwrap().getattr("timezone").unwrap();
                assert!(
                    !hydrated
                        .bind(py)
                        .getattr("tzinfo")
                        .unwrap()
                        .is_instance(&timezone_type)
                        .unwrap()
                );
                assert_eq!(
                    canonical_python_datetime_tz(py, hydrated.bind(py), marker).unwrap(),
                    evidence
                );
            }
        });
    }

    #[test]
    fn unordered_datetime_tz_keeps_offset_only_predecessor_behavior() {
        Python::initialize();
        Python::attach(|py| {
            let authored = py
                .import("datetime")
                .unwrap()
                .getattr("datetime")
                .unwrap()
                .call_method1("fromisoformat", ("2026-11-01T01:30:00-05:00",))
                .unwrap();
            assert_eq!(
                canonical_attribute_value_from_py(py, &authored, ValueType::DateTimeTz, None)
                    .unwrap(),
                AttributeValue::DateTimeTZ("2026-11-01T01:30:00-05:00".into())
            );

            let hydrated = attribute_value_to_py(
                py,
                &AttributeValue::DateTimeTZ("2026-11-01T01:30:00-05:00".into()),
                None,
            )
            .unwrap();
            assert!(
                hydrated
                    .bind(py)
                    .getattr("tzinfo")
                    .unwrap()
                    .getattr("key")
                    .is_err(),
                "legacy unordered hydration must not upgrade offset evidence to ZoneInfo",
            );
            assert!(
                attribute_value_to_py(
                    py,
                    &AttributeValue::DateTimeTZ(
                        "2026-11-01T01:30:00-05:00[America/New_York]".into(),
                    ),
                    None,
                )
                .is_err(),
                "legacy unordered hydration keeps rejecting bracketed named-zone evidence",
            );
        });
    }

    fn classes(
        py: Python<'_>,
        projection: &RuntimeProjection,
    ) -> Vec<(Py<PyType>, Option<Py<PyType>>)> {
        let module = PyModule::from_code(
            py,
            ffi::c_str!(
                r#"
class Complete:
    __slots__ = ("_values", "_iid", "__weakref__")
    __model_form__ = "complete"
    def __init__(self, **values):
        self._values = dict(values)
        self._iid = None
    @property
    def iid(self):
        return self._iid
    def runtime_values(self):
        return self._values
    def initialize_runtime_values(self, values):
        self._values = dict(values)
        self._iid = None
    def attach_runtime_iid(self, iid):
        self._iid = iid

class Attribute(Complete):
    __slots__ = ("_attribute_value",)
    def __init__(self, value):
        super().__init__()
        self._attribute_value = value
    def runtime_attribute_value(self):
        return self._attribute_value

class Reference:
    __slots__ = ("_values", "_iid", "__weakref__")
    __model_form__ = "reference"
    def __init__(self, iid, **values):
        self.initialize_runtime_reference(iid, values)
    @property
    def iid(self):
        return self._iid
    def runtime_values(self):
        return self._values
    def initialize_runtime_reference(self, iid, values):
        self._iid = iid
        self._values = dict(values)
"#
            ),
            ffi::c_str!("projection_models.py"),
            ffi::c_str!("projection_models"),
        )
        .unwrap();
        let builtins = py.import("builtins").unwrap();
        let type_fn = builtins.getattr("type").unwrap();
        projection
            .models()
            .iter()
            .map(|(id, model)| {
                let base = if id.kind() == TypeKind::Attribute {
                    "Attribute"
                } else {
                    "Complete"
                };
                let attrs = PyDict::new(py);
                attrs
                    .set_item("__type_id__", canonical_id(id).unwrap())
                    .unwrap();
                attrs.set_item("__model_form__", "complete").unwrap();
                attrs.set_item("__slots__", PyTuple::empty(py)).unwrap();
                let bases = PyTuple::new(py, [module.getattr(base).unwrap()]).unwrap();
                let complete = type_fn
                    .call1((model.target_name().as_str(), bases, attrs))
                    .unwrap()
                    .cast_into::<PyType>()
                    .unwrap()
                    .unbind();
                let reference = model.reference_read().target_name().map(|name| {
                    let attrs = PyDict::new(py);
                    attrs
                        .set_item("__type_id__", canonical_id(id).unwrap())
                        .unwrap();
                    attrs.set_item("__model_form__", "reference").unwrap();
                    attrs.set_item("__slots__", PyTuple::empty(py)).unwrap();
                    let bases = PyTuple::new(py, [module.getattr("Reference").unwrap()]).unwrap();
                    type_fn
                        .call1((name.as_str(), bases, attrs))
                        .unwrap()
                        .cast_into::<PyType>()
                        .unwrap()
                        .unbind()
                });
                (complete, reference)
            })
            .collect()
    }

    fn struct_classes(py: Python<'_>, projection: &RuntimeProjection) -> Vec<Py<PyType>> {
        let module = PyModule::from_code(
            py,
            ffi::c_str!(
                r#"
class StructBase:
    __slots__ = ()
    def __init__(self, **values):
        for name, value in values.items():
            object.__setattr__(self, name, value)
"#
            ),
            ffi::c_str!("projection_structs.py"),
            ffi::c_str!("projection_structs"),
        )
        .unwrap();
        let builtins = py.import("builtins").unwrap();
        let type_fn = builtins.getattr("type").unwrap();
        projection
            .structs()
            .values()
            .map(|structure| {
                let id = TypeId::new(TypeKind::Struct, structure.id().label().as_str()).unwrap();
                let attrs = PyDict::new(py);
                attrs
                    .set_item("__struct_id__", canonical_id(&id).unwrap())
                    .unwrap();
                attrs
                    .set_item(
                        "__slots__",
                        PyTuple::new(
                            py,
                            structure
                                .fields()
                                .iter()
                                .map(|field| field.target_name().as_str()),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                let bases = PyTuple::new(py, [module.getattr("StructBase").unwrap()]).unwrap();
                type_fn
                    .call1((structure.target_name().as_str(), bases, attrs))
                    .unwrap()
                    .cast_into::<PyType>()
                    .unwrap()
                    .unbind()
            })
            .collect()
    }

    fn install(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let authority = authority(SCHEMA, "python-native.yaml");
        let projection = python_projection(&authority);
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();
        let package = install_projection(
            py,
            &projection_json,
            &semantic,
            &fingerprint,
            classes(py, &projection),
            None,
        )
        .unwrap();
        (projection, package)
    }

    fn install_ordered(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let authority = authority(ORDERED_SCHEMA, "python-ordered-native.yaml");
        let authority_bytes = encode_schema_authority(&authority);
        let projection = python_projection(&authority);
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();
        let package = install_projection(
            py,
            &projection_json,
            &semantic,
            &fingerprint,
            classes(py, &projection),
            Some(&authority_bytes),
        )
        .unwrap();
        (projection, package)
    }

    fn install_batch(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let authority = authority(BATCH_SCHEMA, "python-batch-native.yaml");
        let authority_bytes = encode_schema_authority(&authority);
        let projection = python_projection(&authority);
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();
        let package = install_projection(
            py,
            &projection_json,
            &semantic,
            &fingerprint,
            classes(py, &projection),
            Some(&authority_bytes),
        )
        .unwrap();
        (projection, package)
    }

    fn origin_person_document(iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "person",
            "attributes": {
                "tag": [{"value": "source-tag"}]
            }
        })
    }

    fn origin_gathering_document(iid: &str, player_iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "gathering",
            "attributes": {},
            "role_players": [{
                "role_name": "participant",
                "player_iid": player_iid,
                "player_type_name": "person",
                "attributes": {
                    "tag": [{"value": "source-tag"}]
                }
            }]
        })
    }

    fn origin_container_document(iid: &str, player_iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "container",
            "attributes": {},
            "role_players": [{
                "role_name": "item",
                "player_iid": player_iid,
                "player_type_name": "gathering",
                "attributes": {}
            }]
        })
    }

    fn batch_person(
        py: Python<'_>,
        package: &InstalledPackage,
        identifier: &str,
        tag: &str,
        iid: Option<&str>,
    ) -> Py<PyAny> {
        let person_id = package.type_by_label("person", TypeKind::Entity).unwrap();
        let identifier_id = package
            .type_by_label("identifier", TypeKind::Attribute)
            .unwrap();
        let tag_id = package.type_by_label("tag", TypeKind::Attribute).unwrap();
        let identifier = package
            .class(identifier_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call1((identifier,))
            .unwrap();
        let tag = package
            .class(tag_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call1((tag,))
            .unwrap();
        let kwargs = PyDict::new(py);
        kwargs.set_item("identifier", identifier).unwrap();
        kwargs
            .set_item("tag", PyTuple::new(py, [tag]).unwrap())
            .unwrap();
        let person = package
            .class(person_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap();
        if let Some(iid) = iid {
            person.call_method1("attach_runtime_iid", (iid,)).unwrap();
        }
        person.unbind()
    }

    fn batch_membership(
        py: Python<'_>,
        package: &InstalledPackage,
        identifier: &str,
        player: &Py<PyAny>,
        iid: Option<&str>,
    ) -> Py<PyAny> {
        let membership_id = package
            .type_by_label("membership", TypeKind::Relation)
            .unwrap();
        let identifier_id = package
            .type_by_label("identifier", TypeKind::Attribute)
            .unwrap();
        let identifier = package
            .class(identifier_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call1((identifier,))
            .unwrap();
        let kwargs = PyDict::new(py);
        kwargs.set_item("identifier", identifier).unwrap();
        kwargs.set_item("member", player.bind(py)).unwrap();
        let relation = package
            .class(membership_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap();
        if let Some(iid) = iid {
            relation.call_method1("attach_runtime_iid", (iid,)).unwrap();
        }
        relation.unbind()
    }

    fn legacy_person(
        py: Python<'_>,
        package: &InstalledPackage,
        identifier: &str,
        iid: Option<&str>,
    ) -> Py<PyAny> {
        let person_id = package.type_by_label("person", TypeKind::Entity).unwrap();
        let identifier_id = package
            .type_by_label("identifier", TypeKind::Attribute)
            .unwrap();
        let identifier = package
            .class(identifier_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call1((identifier,))
            .unwrap();
        let kwargs = PyDict::new(py);
        kwargs.set_item("identifier", identifier).unwrap();
        let person = package
            .class(person_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap();
        if let Some(iid) = iid {
            person.call_method1("attach_runtime_iid", (iid,)).unwrap();
        }
        person.unbind()
    }

    fn legacy_membership(
        py: Python<'_>,
        package: &InstalledPackage,
        player: &Py<PyAny>,
        iid: Option<&str>,
    ) -> Py<PyAny> {
        let membership_id = package
            .type_by_label("membership", TypeKind::Relation)
            .unwrap();
        let kwargs = PyDict::new(py);
        kwargs.set_item("member", player.bind(py)).unwrap();
        let membership = package
            .class(membership_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap();
        if let Some(iid) = iid {
            membership
                .call_method1("attach_runtime_iid", (iid,))
                .unwrap();
        }
        membership.unbind()
    }

    fn legacy_person_document(iid: &str, identifier: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "person",
            "attributes": {"identifier": [{"value": identifier}]},
        })
    }

    fn legacy_membership_document(
        iid: &str,
        player_iid: &str,
        player_identifier: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "membership",
            "attributes": {},
            "role_players": [{
                "role_name": "member",
                "player_iid": player_iid,
                "player_type_name": "person",
                "attributes": {"identifier": [{"value": player_identifier}]},
            }],
        })
    }

    fn batch_person_document(
        ordinal: usize,
        iid: &str,
        identifier: &str,
        tag: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "ordinal": ordinal,
            "_iid": iid,
            "_type": "person",
            "attributes": {
                "identifier": [{"value": identifier}],
                "tag": [{"value": tag}],
            },
        })
    }

    fn batch_membership_document(
        ordinal: usize,
        iid: &str,
        identifier: &str,
        player_iid: &str,
        player_identifier: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "ordinal": ordinal,
            "_iid": iid,
            "_type": "membership",
            "attributes": {"identifier": [{"value": identifier}]},
            "role_players": [{
                "role": "membership:member",
                "iid": player_iid,
                "type_name": "person",
                "attributes": {
                    "identifier": [{"value": player_identifier}],
                },
            }],
        })
    }

    fn entity_batch_responses(operation: ProjectedBatchOperation) -> Vec<BatchResponse> {
        let mut responses = Vec::new();
        if operation == ProjectedBatchOperation::Put {
            responses.push(BatchResponse::Documents(vec![]));
        }
        if operation == ProjectedBatchOperation::Delete {
            responses.push(BatchResponse::Documents(vec![
                serde_json::json!({"ordinal": 1}),
                serde_json::json!({"ordinal": 0}),
            ]));
            return responses;
        }
        responses.push(BatchResponse::Documents(vec![
            serde_json::json!({"ordinal": 1, "iid": "0x21"}),
            serde_json::json!({"ordinal": 0, "iid": "0x20"}),
        ]));
        responses.push(BatchResponse::Documents(vec![
            batch_person_document(1, "0x21", "person-1", "normalized-1"),
            batch_person_document(0, "0x20", "person-0", "normalized-0"),
        ]));
        responses
    }

    fn entity_insert_responses(count: usize) -> Vec<BatchResponse> {
        vec![
            BatchResponse::Documents(
                (0..count)
                    .rev()
                    .map(|ordinal| {
                        serde_json::json!({
                            "ordinal": ordinal,
                            "iid": format!("0x2{ordinal}"),
                        })
                    })
                    .collect(),
            ),
            BatchResponse::Documents(
                (0..count)
                    .rev()
                    .map(|ordinal| {
                        batch_person_document(
                            ordinal,
                            &format!("0x2{ordinal}"),
                            &format!("person-{ordinal}"),
                            &format!("normalized-{ordinal}"),
                        )
                    })
                    .collect(),
            ),
        ]
    }

    fn relation_batch_responses(operation: ProjectedBatchOperation) -> Vec<BatchResponse> {
        if operation == ProjectedBatchOperation::Delete {
            return vec![BatchResponse::Documents(vec![
                serde_json::json!({"ordinal": 1}),
                serde_json::json!({"ordinal": 0, "iid": "0x30"}),
            ])];
        }
        vec![
            BatchResponse::Documents(vec![
                serde_json::json!({
                    "kind": 1,
                    "ordinal": 1,
                    "reference_ordinal": 1,
                    "iid": "0x11",
                    "type": "person",
                }),
                serde_json::json!({
                    "kind": 1,
                    "ordinal": 0,
                    "reference_ordinal": 0,
                    "iid": "0x10",
                    "type": "person",
                }),
            ]),
            BatchResponse::Documents(vec![
                serde_json::json!({"ordinal": 1, "iid": "0x31"}),
                serde_json::json!({"ordinal": 0, "iid": "0x30"}),
            ]),
            BatchResponse::Documents(vec![
                batch_membership_document(1, "0x31", "membership-1", "0x11", "player-1"),
                batch_membership_document(0, "0x30", "membership-0", "0x10", "player-0"),
            ]),
        ]
    }

    fn assert_successor_batch_route_fingerprint(
        kind: TypeKind,
        operation: ProjectedBatchOperation,
        calls: &[(String, GivenRowsSpec)],
    ) {
        let label = if kind == TypeKind::Entity {
            "person"
        } else {
            "membership"
        };
        let has_prerequisite = (kind == TypeKind::Relation
            && operation != ProjectedBatchOperation::Delete)
            || operation == ProjectedBatchOperation::Put;
        let mutation_index = usize::from(has_prerequisite);
        let (mutation, rows) = &calls[mutation_index];

        match operation {
            ProjectedBatchOperation::Insert => {
                assert!(
                    mutation.contains(&format!("insert\n$thing isa {label};")),
                    "insert batch reached the wrong mutation compiler: {mutation}",
                );
                assert!(!mutation.contains("\nput\n"));
                assert_eq!(rows.variables, ["ordinal"]);
                assert_eq!(
                    rows.rows,
                    [vec![GivenValue::Integer(0)], vec![GivenValue::Integer(1)],],
                );
            }
            ProjectedBatchOperation::Put => {
                assert_eq!(mutation.matches("\nput\n").count(), 1);
                assert!(mutation.contains(&format!("$thing isa {label}")));
                assert!(mutation.contains("has identifier == $put-key-0"));
                assert_eq!(rows.variables, ["ordinal", "put-key-0"]);
                let prefix = if kind == TypeKind::Entity {
                    "person"
                } else {
                    "membership"
                };
                assert_eq!(
                    rows.rows,
                    [
                        vec![
                            GivenValue::Integer(0),
                            GivenValue::String(format!("{prefix}-0")),
                        ],
                        vec![
                            GivenValue::Integer(1),
                            GivenValue::String(format!("{prefix}-1")),
                        ],
                    ],
                );
            }
            ProjectedBatchOperation::Update => {
                assert!(mutation.contains(&format!("$thing isa! {label};")));
                assert!(mutation.contains("$actual-iid == $target-iid;"));
                assert!(mutation.contains("has $old-attribute-0 of $thing"));
                assert!(!mutation.contains("\nput\n"));
                if kind == TypeKind::Relation {
                    assert!(
                        mutation
                            .contains("delete try { links (member: $old-player-0) of $thing; }",),
                        "relation update did not clear the authored role: {mutation}",
                    );
                }
                assert_eq!(rows.variables, ["ordinal", "target-iid"]);
                let base = if kind == TypeKind::Entity { 0x20 } else { 0x30 };
                assert_eq!(
                    rows.rows,
                    [
                        vec![
                            GivenValue::Integer(0),
                            GivenValue::String(format!("0x{base:x}")),
                        ],
                        vec![
                            GivenValue::Integer(1),
                            GivenValue::String(format!("0x{:x}", base + 1)),
                        ],
                    ],
                );
            }
            ProjectedBatchOperation::Delete => {
                assert!(mutation.contains(&format!("$thing isa! {label};")));
                assert!(mutation.contains("delete try { $thing; }"));
                assert!(!mutation.contains("\nput\n"));
                assert_eq!(rows.variables, ["ordinal", "target-iid"]);
                let base = if kind == TypeKind::Entity { 0x20 } else { 0x30 };
                assert_eq!(
                    rows.rows,
                    [
                        vec![
                            GivenValue::Integer(0),
                            GivenValue::String(format!("0x{base:x}")),
                        ],
                        vec![
                            GivenValue::Integer(1),
                            GivenValue::String(format!("0x{:x}", base + 1)),
                        ],
                    ],
                );
            }
        }

        if has_prerequisite {
            let (prerequisite, rows) = &calls[0];
            assert_eq!(
                &rows.variables[..5],
                [
                    "kind",
                    "ordinal",
                    "reference-ordinal",
                    "route",
                    "wanted-iid"
                ],
            );
            if operation == ProjectedBatchOperation::Put {
                assert!(prerequisite.contains("$kind == 0;"));
                assert!(prerequisite.contains("has identifier == $put-key-0"));
                assert_eq!(
                    rows.rows
                        .iter()
                        .filter(|row| row[0] == GivenValue::Integer(0))
                        .count(),
                    2,
                );
            }
            if kind == TypeKind::Relation {
                assert!(prerequisite.contains("$kind == 1;"));
                assert_eq!(
                    rows.rows
                        .iter()
                        .filter(|row| row[0] == GivenValue::Integer(1))
                        .count(),
                    2,
                );
            }
        }

        if operation != ProjectedBatchOperation::Delete {
            let (attach, rows) = calls.last().expect("non-delete batch has an attach stage");
            assert!(attach.contains(&format!("$thing isa! {label};")));
            assert_eq!(&rows.variables[..2], ["ordinal", "target-iid"]);
            assert!(rows.rows.iter().any(|row| {
                row[0] == GivenValue::Integer(0)
                    && row[1]
                        == GivenValue::String(if kind == TypeKind::Entity {
                            "0x20".to_owned()
                        } else {
                            "0x30".to_owned()
                        })
            }));
            if kind == TypeKind::Relation {
                assert!(attach.contains("$thing links (member:"));
                assert!(
                    rows.variables
                        .iter()
                        .any(|variable| variable.starts_with("player-iid-")),
                );
            }
        }
    }

    fn prepared_batch_facades(
        py: Python<'_>,
        package: &InstalledPackage,
        facades: Vec<Py<PyAny>>,
    ) -> Vec<PreparedBatchFacade> {
        let person_id = package.type_by_label("person", TypeKind::Entity).unwrap();
        let slots = package.batch_slots(person_id).unwrap();
        facades
            .into_iter()
            .map(|instance| PreparedBatchFacade {
                origin: package.facade_origins.prepare(instance.bind(py)).unwrap(),
                snapshot: slots.snapshot(py, instance.bind(py)).unwrap(),
                instance,
            })
            .collect()
    }

    fn hydration_pool_for_prepared(
        py: Python<'_>,
        facades: &[PreparedBatchFacade],
    ) -> BatchHydrationPool {
        let staged = facades
            .iter()
            .map(|facade| StagedBatchFacade {
                instance: facade.instance.clone_ref(py),
                snapshot: facade.snapshot.clone_ref(py),
            })
            .collect::<Vec<_>>();
        BatchHydrationPool::try_new(&staged).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_batch_with_materialization_probe(
        py: Python<'_>,
        package: Arc<InstalledPackage>,
        type_id: TypeId,
        database: Option<Arc<Database>>,
        transaction: Option<TransactionContext>,
        successor_batch_marker: Option<Arc<AtomicBool>>,
        runtime: Arc<ProviderRuntimeOwner>,
        instances: Vec<Py<PyAny>>,
        probe: BatchMaterializationProbe,
    ) -> PyResult<Py<PyAny>> {
        provider_block_on_with_gil(py, runtime.as_ref(), move |py| {
            Box::pin(async move {
                let slots = package.batch_slots(&type_id)?;
                let mut rows = reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;
                let mut staged_facades =
                    reserved_binding_rows(instances.len()).map_err(py_sdk_diagnostic)?;
                for (ordinal, instance) in instances.into_iter().enumerate() {
                    let instance = instance.bind(py);
                    ensure_batch_instance_at(py, package.as_ref(), &type_id, instance, ordinal)?;
                    let snapshot = slots.snapshot(py, instance).map_err(|_| {
                        py_sdk_diagnostic(batch_input_shape_diagnostic(
                            "generated_model_layout_mismatch",
                            "The exact generated model no longer has its installed runtime slots",
                            &projected_batch_row_path(ordinal),
                        ))
                    })?;
                    rows.push(ProjectedBatchRow::Create(project_create(
                        py,
                        package.as_ref(),
                        &type_id,
                        instance,
                    )?));
                    staged_facades.push(StagedBatchFacade {
                        snapshot,
                        instance: instance.clone().unbind(),
                    });
                }
                let batch = ProjectedBatch::try_new(
                    package.projection.as_ref(),
                    type_id,
                    ProjectedBatchOperation::Insert,
                    rows,
                )
                .map_err(py_sdk_diagnostic)?;
                let _gc_guard = SuccessorBatchGcGuard::disable(py);
                let hydration_pool =
                    BatchHydrationPool::try_new(&staged_facades).map_err(py_sdk_diagnostic)?;
                package.validate_batch_hydration_layouts(py).map_err(|_| {
                    py_sdk_diagnostic(batch_input_shape_diagnostic(
                        "generated_model_layout_mismatch",
                        "An installed successor hydration class changed before execution",
                        &projected_batch_rows_path(),
                    ))
                })?;
                validate_staged_batch_facades_after_lowering(py, slots.as_ref(), &staged_facades)?;
                let mut facades =
                    reserved_binding_rows(staged_facades.len()).map_err(py_sdk_diagnostic)?;
                for staged in staged_facades {
                    let instance = staged.instance.bind(py);
                    facades.push(PreparedBatchFacade {
                        origin: package.facade_origins.prepare(instance)?,
                        snapshot: staged.snapshot,
                        instance: instance.clone().unbind(),
                    });
                }

                let empty = BatchMappedOutput::Empty(fallible_python_empty_list(py)?);
                let binding_error = Mutex::new(None);
                let mapper_package = Arc::clone(&package);
                let mapper_error = &binding_error;
                let mapper_pool = &hydration_pool;
                let mapper_marker = successor_batch_marker.clone();
                let mapper = move |result| {
                    if let Some(marker) = &mapper_marker {
                        marker.store(true, Ordering::Release);
                    }
                    materialize_projected_batch_with_probe(
                        py,
                        mapper_package,
                        slots,
                        facades,
                        result,
                        mapper_error,
                        mapper_pool,
                        probe,
                    )
                    .map(BatchMappedOutput::Publication)
                };
                let executor = ProjectedBatchExecutor::new(package.projection.as_ref());
                let control = ProjectedBatchInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                );
                let result = match (&database, &transaction) {
                    (Some(database), None) => {
                        executor
                            .execute_mapped(database.as_ref(), &batch, control, empty, mapper)
                            .await
                    }
                    (None, Some(transaction)) => {
                        executor
                            .execute_in_transaction_mapped(
                                transaction,
                                &batch,
                                control,
                                empty,
                                mapper,
                            )
                            .await
                    }
                    _ => Err(SdkExecutionDiagnostic::internal_failure()),
                };
                let output = match result {
                    Ok(mapped) => Ok(mapped.finish()),
                    Err(diagnostic) => match take_binding_materialization_error(&binding_error) {
                        Some(error) => Err(error),
                        None => Err(py_sdk_diagnostic(diagnostic)),
                    },
                };
                drop(hydration_pool);
                output
            })
        })
    }

    fn projected_batch_people(
        py: Python<'_>,
        package: &InstalledPackage,
        count: usize,
    ) -> Vec<ProjectedThing> {
        let person_id = package
            .type_by_label("person", TypeKind::Entity)
            .unwrap()
            .clone();
        (0..count)
            .map(|ordinal| {
                let facade = batch_person(
                    py,
                    package,
                    &format!("provider-{ordinal}"),
                    &format!("normalized-{ordinal}"),
                    Some(&format!("0x2{ordinal}")),
                );
                let values = facade
                    .bind(py)
                    .call_method0("runtime_values")
                    .unwrap()
                    .cast_into::<PyDict>()
                    .unwrap();
                project_hydrated_thing(
                    py,
                    package,
                    &person_id,
                    &values,
                    Some(&format!("0x2{ordinal}")),
                )
                .unwrap()
            })
            .collect()
    }

    fn origin_manager(
        package: Arc<InstalledPackage>,
        type_id: TypeId,
        database: Option<Arc<Database>>,
        transaction: Option<TransactionContext>,
        runtime: Arc<ProviderRuntimeOwner>,
    ) -> PyProjectedModelManager {
        let successor_batch_marker = transaction
            .as_ref()
            .map(|_| Arc::new(AtomicBool::new(false)));
        PyProjectedModelManager {
            package,
            type_id,
            database,
            transaction,
            successor_batch_marker,
            runtime,
            filters: vec![],
            compatibility_filter: None,
        }
    }

    #[test]
    fn canonical_manager_filter_fences_tokens_domains_and_persists() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let membership_id = package
                .type_by_label("membership", TypeKind::Relation)
                .unwrap()
                .clone();
            let identifier_id = package
                .type_by_label("identifier", TypeKind::Attribute)
                .unwrap()
                .clone();
            let tag_id = package
                .type_by_label("tag", TypeKind::Attribute)
                .unwrap()
                .clone();
            let switch_id = package
                .type_by_label("switch", TypeKind::Entity)
                .unwrap()
                .clone();
            let flag_id = package
                .type_by_label("flag", TypeKind::Attribute)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let membership_class = package
                .class(&membership_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let switch_class = package
                .class(&switch_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let identifier = package
                .class(&identifier_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call1(("person-1",))
                .unwrap();
            let tag = package
                .class(&tag_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call1(("tag-1",))
                .unwrap();
            let flag = package
                .class(&flag_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call1((true,))
                .unwrap();
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                None,
                None,
                Arc::new(ProviderRuntimeOwner::new().unwrap()),
            );
            let field_metadata = |owner: &TypeId, name: &str| {
                let field = package
                    .projection
                    .projection()
                    .models()
                    .get(owner)
                    .unwrap()
                    .query_tokens()
                    .fields()
                    .values()
                    .find(|field| field.target_name().as_str() == name)
                    .unwrap();
                String::from_utf8(to_canonical_json(field).unwrap()).unwrap()
            };
            let person_identifier_metadata = field_metadata(&person_id, "identifier");
            let membership_identifier_metadata = field_metadata(&membership_id, "identifier");
            let switch_flag_metadata = field_metadata(&switch_id, "flag");
            let filter = manager.canonical_filter().unwrap();
            let sibling = filter
                .where_field(
                    py,
                    person_class.clone_ref(py),
                    "identifier",
                    &person_identifier_metadata,
                    "eq",
                    identifier.clone(),
                )
                .unwrap();
            assert_eq!(filter.filter.len(), 0);
            assert_eq!(sibling.filter.len(), 1);
            let switch_filter =
                ProjectedManagerFilter::try_new(&package.projection, switch_id).unwrap();
            let switch_filter = PyProjectedManagerFilter {
                package: Arc::clone(&package),
                database: None,
                transaction: None,
                runtime: Arc::new(ProviderRuntimeOwner::new().unwrap()),
                filter: switch_filter,
            };

            let cases = [
                (
                    switch_filter
                        .where_field(py, switch_class, "flag", &switch_flag_metadata, "gt", flag)
                        .err()
                        .expect("boolean ordering must fail"),
                    "invalid_input",
                    "invalid_operator_for_type",
                ),
                (
                    filter
                        .where_field(
                            py,
                            membership_class,
                            "identifier",
                            &membership_identifier_metadata,
                            "eq",
                            identifier,
                        )
                        .err()
                        .expect("wrong-owner token must fail"),
                    "integrity",
                    "field_owner_mismatch",
                ),
                (
                    filter
                        .where_field(
                            py,
                            person_class,
                            "identifier",
                            &person_identifier_metadata,
                            "eq",
                            tag,
                        )
                        .err()
                        .expect("wrong scalar domain must fail"),
                    "invalid_input",
                    "wrong_scalar_domain",
                ),
                (
                    filter
                        .reject_unissued_field()
                        .err()
                        .expect("unissued token must fail"),
                    "integrity",
                    "generated_token_package_mismatch",
                ),
            ];
            for (error, category, code) in cases {
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    category,
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    code,
                );
            }
        });
    }

    #[test]
    fn write_transaction_manager_preserves_legacy_reads_and_rejects_canonical_filter_pre_io() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let (backend, state) =
                OriginRecordingBackend::new(vec![QueryResult::Documents(vec![])]);
            let database = Arc::new(Database::with_backend(
                Box::new(backend),
                "canonical-write-target",
            ));
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let transaction = runtime
                .block_on(database.transaction_context(TxType::Write))
                .expect("write transaction should open");
            let context = PyRustTransactionContext::from_parts_for_test(
                transaction.clone(),
                Arc::clone(&runtime),
            );
            let projection = PyRuntimeProjection {
                package: Arc::clone(&package),
            };
            let manager = projection
                .manager_for_transaction(py, person_class, &context)
                .expect("released manager should bind to a write transaction");

            assert!(manager.compatibility_filter.is_none());
            let error = manager
                .canonical_filter()
                .err()
                .expect("canonical filters require a borrowed read transaction");
            let diagnostic = error.value(py);
            assert_eq!(
                diagnostic
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "invalid_input",
            );
            assert_eq!(
                diagnostic
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "borrowed_target_not_read_only",
            );
            assert!(state.lock().unwrap().queries.is_empty());

            let values = manager
                .all(py)
                .expect("released write-transaction manager reads remain on the legacy route");
            assert_eq!(values.bind(py).cast::<PyList>().unwrap().len(), 0);
            {
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 1);
            }
            runtime
                .block_on(transaction.close())
                .expect("write transaction should close");
        });
    }

    fn thing_query(
        package: &InstalledPackage,
        type_name: &str,
    ) -> (
        Arc<type_bridge_orm::_registry::DescriptorRegistry>,
        type_bridge_orm::ValidatedMatchRequest,
    ) {
        let registry = Arc::new(package.projection.match_registry().unwrap());
        let session = SessionHandle::new(Arc::clone(&registry));
        let thing = session.exact(type_name).unwrap();
        let shape = session.positional([thing.one()]).unwrap();
        let query = session.query(shape).unwrap();
        let validated = query
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::ExactlyOne,
            )
            .unwrap();
        (registry, validated)
    }

    fn person_query(
        package: &InstalledPackage,
    ) -> (
        Arc<type_bridge_orm::_registry::DescriptorRegistry>,
        type_bridge_orm::ValidatedMatchRequest,
    ) {
        thing_query(package, "person")
    }

    fn query_budget() -> PyQueryInvocationBudget {
        let resources = QueryExecutionResourceLimits::default();
        PyQueryInvocationBudget::from_parts(
            resources,
            QueryExecutionDeadline::for_limits(resources),
            AnswerCancellation::default(),
        )
    }

    fn hydrate_first_query_thing(
        py: Python<'_>,
        package: &InstalledPackage,
        handle: crate::validated_result_runtime::PyValidatedMatchResultHandle,
    ) -> Py<PyAny> {
        let handle = Py::new(py, handle).unwrap();
        let row = handle.bind(py).call_method1("row", (0,)).unwrap();
        let slot = row.call_method1("slot", (0,)).unwrap();
        let thing = slot.call_method1("thing", (0,)).unwrap();
        let thing = thing
            .extract::<PyRef<'_, PyValidatedMatchThingHandle>>()
            .unwrap();
        hydrate_validated_thing(py, package, &thing).unwrap()
    }

    fn gathering_with_player<'py>(
        py: Python<'py>,
        package: &InstalledPackage,
        gathering_id: &TypeId,
        player: &Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        relation_with_player(py, package, gathering_id, "participant", player)
    }

    fn relation_with_player<'py>(
        py: Python<'py>,
        package: &InstalledPackage,
        relation_id: &TypeId,
        role: &str,
        player: &Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        let kwargs = PyDict::new(py);
        kwargs
            .set_item(role, PyTuple::new(py, [player]).unwrap())
            .unwrap();
        package
            .class(relation_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap()
    }

    #[test]
    fn install_is_canonical_tamper_evident_and_requires_exact_coverage() {
        Python::initialize();
        Python::attach(|py| {
            let authority = authority(SCHEMA, "python-native.yaml");
            let projection = python_projection(&authority);
            let projection_json =
                String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic =
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap();
            let fingerprint =
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap();
            install_projection(
                py,
                &projection_json,
                &semantic,
                &fingerprint,
                classes(py, &projection),
                None,
            )
            .unwrap();

            let mut missing = classes(py, &projection);
            missing.pop();
            assert!(
                install_projection(py, &projection_json, &semantic, &fingerprint, missing, None,)
                    .is_err()
            );

            let mut tampered: serde_json::Value = serde_json::from_str(&projection_json).unwrap();
            tampered["models"][0]["target_name"] = serde_json::json!("Tampered");
            let tampered = String::from_utf8(to_canonical_json(&tampered).unwrap()).unwrap();
            assert!(
                install_projection(
                    py,
                    &tampered,
                    &semantic,
                    &fingerprint,
                    classes(py, &projection),
                    None,
                )
                .is_err()
            );
        });
    }

    #[test]
    fn install_rejects_a_foreign_binding_target_before_registration() {
        let authority = authority(SCHEMA, "typescript-native.yaml");
        let emitter = TypeScriptEmitter::new();
        let projection = project(
            authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();

        Python::initialize();
        Python::attach(|py| {
            let error =
                install_projection(py, &projection_json, &semantic, &fingerprint, vec![], None)
                    .err()
                    .expect("a TypeScript projection must not install as Python");
            assert!(
                error
                    .to_string()
                    .contains("runtime projection does not target Python")
            );
        });
    }

    #[test]
    fn whole_create_rejects_a_shape_compatible_foreign_fieldless_instance() {
        Python::initialize();
        Python::attach(|py| {
            let (projection, local_package) = install(py);
            let projection_json =
                String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic =
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap();
            let fingerprint =
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap();
            let foreign_package = install_projection(
                py,
                &projection_json,
                &semantic,
                &fingerprint,
                classes(py, &projection),
                None,
            )
            .unwrap();
            let event_id = local_package
                .type_by_label("event", TypeKind::Relation)
                .unwrap()
                .clone();
            let local_class = local_package
                .class(&event_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let local_instance = local_class.bind(py).call0().unwrap();
            let runtime = PyRuntimeProjection {
                package: local_package,
            };
            runtime
                .validate_create(py, local_class.clone_ref(py), local_instance)
                .unwrap();

            let foreign_instance = foreign_package
                .class(&event_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call0()
                .unwrap();
            let error = runtime
                .validate_create(py, local_class, foreign_instance)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "generated_token_package_mismatch"
            );
        });
    }

    #[test]
    fn canonical_create_codec_round_trips_exact_python_nominal_value() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let absent_instance = class.bind(py).call0().unwrap();
            let present_empty_args = PyDict::new(py);
            present_empty_args
                .set_item("tag", PyTuple::empty(py))
                .unwrap();
            let present_empty_instance =
                class.bind(py).call((), Some(&present_empty_args)).unwrap();
            let runtime = PyRuntimeProjection { package };
            let absent_bytes = runtime
                .encode_create(py, class.clone_ref(py), absent_instance)
                .unwrap();
            let present_empty_bytes = runtime
                .encode_create(py, class.clone_ref(py), present_empty_instance)
                .unwrap();
            assert_ne!(absent_bytes.as_bytes(), present_empty_bytes.as_bytes());

            let absent = runtime
                .decode_create(py, class.clone_ref(py), &absent_bytes)
                .unwrap();
            assert!(absent.bind(py).is_instance(class.bind(py)).unwrap());
            assert!(absent.bind(py).getattr("iid").unwrap().is_none());
            let absent_values = absent
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            assert!(absent_values.get_item("tag").unwrap().is_none());

            let present_empty = runtime
                .decode_create(py, class.clone_ref(py), &present_empty_bytes)
                .unwrap();
            let present_empty_values = present_empty
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            assert_eq!(
                present_empty_values
                    .get_item("tag")
                    .unwrap()
                    .unwrap()
                    .len()
                    .unwrap(),
                0
            );
        });
    }

    #[test]
    fn canonical_attribute_codec_round_trips_exact_python_nominal_value() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let identifier_id = package
                .type_by_label("identifier", TypeKind::Attribute)
                .unwrap()
                .clone();
            let identifier_class = package
                .class(&identifier_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let identifier = identifier_class.bind(py).call1(("person-1",)).unwrap();
            let runtime = PyRuntimeProjection { package };

            let bytes = runtime
                .encode_attribute(py, identifier_class.clone_ref(py), identifier)
                .unwrap();
            let controlled_identifier = identifier_class.bind(py).call1(("person-1",)).unwrap();
            let controlled_bytes = runtime
                .encode_record_controlled(
                    py,
                    "attribute",
                    identifier_class.clone_ref(py),
                    controlled_identifier,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(controlled_bytes.as_bytes(), bytes.as_bytes());
            let controlled = runtime
                .decode_record_controlled(
                    py,
                    "attribute",
                    identifier_class.clone_ref(py),
                    &bytes,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(
                controlled
                    .bind(py)
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );
            let cancellation = PyQueryCancellation::new();
            cancellation.cancel();
            let cancelled_identifier = identifier_class.bind(py).call1(("person-1",)).unwrap();
            let error = runtime
                .encode_record_controlled(
                    py,
                    "attribute",
                    identifier_class.clone_ref(py),
                    cancelled_identifier,
                    Some(&cancellation),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "projected_codec_cancelled"
            );
            let decoded = runtime
                .decode_attribute(py, identifier_class.clone_ref(py), &bytes)
                .unwrap();
            assert!(
                decoded
                    .bind(py)
                    .is_instance(identifier_class.bind(py))
                    .unwrap()
            );
            assert_eq!(
                decoded
                    .bind(py)
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );

            let flag_id = runtime
                .package
                .type_by_label("flag", TypeKind::Attribute)
                .unwrap();
            let flag_class = runtime
                .package
                .class(flag_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            assert!(runtime.decode_attribute(py, flag_class, &bytes).is_err());
        });
    }

    #[test]
    fn canonical_reference_snapshot_and_archive_round_trip_exact_python_values() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let complete_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let reference_class = package
                .class(&person_id, ProjectedModelForm::Reference)
                .unwrap()
                .clone_ref(py);
            let snapshot = batch_person(
                py,
                package.as_ref(),
                "archive-person",
                "archive-tag",
                Some("0xabc"),
            );
            let snapshot_values = snapshot
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            let reference_values = PyDict::new(py);
            reference_values
                .set_item(
                    "identifier",
                    snapshot_values.get_item("identifier").unwrap().unwrap(),
                )
                .unwrap();
            let reference = reference_class
                .bind(py)
                .call((py.None(),), Some(&reference_values))
                .unwrap();
            let runtime = PyRuntimeProjection { package };

            let reference_bytes = runtime
                .encode_reference(py, reference_class.clone_ref(py), reference)
                .unwrap();
            let decoded_reference = runtime
                .decode_reference(py, reference_class.clone_ref(py), &reference_bytes)
                .unwrap();
            assert!(
                decoded_reference
                    .bind(py)
                    .is_instance(reference_class.bind(py))
                    .unwrap()
            );
            assert!(decoded_reference.bind(py).getattr("iid").unwrap().is_none());

            let snapshot_bytes = runtime
                .encode_snapshot(py, complete_class.clone_ref(py), snapshot.bind(py).clone())
                .unwrap();
            let decoded_snapshot = runtime
                .decode_snapshot(py, complete_class.clone_ref(py), &snapshot_bytes)
                .unwrap();
            assert!(
                decoded_snapshot
                    .bind(py)
                    .is_instance(complete_class.bind(py))
                    .unwrap()
            );
            assert_eq!(
                decoded_snapshot
                    .bind(py)
                    .getattr("iid")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0xabc"
            );

            let expected = vec![
                reference_bytes.as_bytes().to_vec(),
                snapshot_bytes.as_bytes().to_vec(),
            ];
            let archive = runtime
                .encode_archive(py, vec![reference_bytes.unbind(), snapshot_bytes.unbind()])
                .unwrap();
            let decoded = runtime.decode_archive(py, &archive).unwrap();
            let actual = decoded
                .iter()
                .map(|value| value.cast_into::<PyBytes>().unwrap().as_bytes().to_vec())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);

            let error_code = |error: PyErr| {
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap()
            };
            let cancellation = PyQueryCancellation::new();
            cancellation.cancel();
            let cancelled = runtime
                .decode_archive_controlled(
                    py,
                    &archive,
                    Some(&cancellation),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error_code(cancelled), "projected_codec_cancelled");
            let input_limited = runtime
                .decode_archive_controlled(
                    py,
                    &archive,
                    None,
                    None,
                    Some(archive.as_bytes().len() - 1),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error_code(input_limited), "projected_codec_input_limit");
            let depth_limited = runtime
                .decode_archive_controlled(
                    py,
                    &archive,
                    None,
                    None,
                    None,
                    None,
                    Some(1),
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error_code(depth_limited), "projected_codec_depth_limit");
            let timed_out = runtime
                .decode_archive_controlled(
                    py,
                    &archive,
                    None,
                    Some(0),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error_code(timed_out), "projected_codec_deadline_exceeded");
            let records = expected
                .iter()
                .map(|bytes| PyBytes::new(py, bytes).unbind())
                .collect::<Vec<_>>();
            let member_limited = runtime
                .encode_archive_controlled(py, records, None, None, None, None, None, Some(1), None)
                .unwrap_err();
            assert_eq!(error_code(member_limited), "projected_codec_member_limit");
            let records = expected
                .iter()
                .map(|bytes| PyBytes::new(py, bytes).unbind())
                .collect::<Vec<_>>();
            let output_limited = runtime
                .encode_archive_controlled(
                    py,
                    records,
                    None,
                    None,
                    None,
                    Some(archive.as_bytes().len() - 1),
                    None,
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error_code(output_limited), "projected_codec_output_limit");
        });
    }

    #[test]
    fn canonical_struct_codec_round_trips_the_exact_python_class() {
        Python::initialize();
        Python::attach(|py| {
            let authority = authority(STRUCT_SCHEMA, "python-struct-native.yaml");
            let authority_bytes = encode_schema_authority(&authority);
            let projection = python_projection(&authority);
            let projection_json =
                String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic =
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap();
            let fingerprint =
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap();
            let structures = struct_classes(py, &projection);
            let structure = structures[0].clone_ref(py);
            let package = install_projection_with_structs(
                py,
                &projection_json,
                &semantic,
                &fingerprint,
                classes(py, &projection),
                structures,
                Some(&authority_bytes),
            )
            .unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("points", 42_i64).unwrap();
            kwargs.set_item("note", "exact").unwrap();
            let value = structure.bind(py).call((), Some(&kwargs)).unwrap();
            let runtime = PyRuntimeProjection { package };
            let bytes = runtime
                .encode_struct(py, structure.clone_ref(py), value)
                .unwrap();
            let decoded = runtime
                .decode_struct(py, structure.clone_ref(py), &bytes)
                .unwrap();

            assert!(decoded.bind(py).is_instance(structure.bind(py)).unwrap());
            assert_eq!(
                decoded
                    .bind(py)
                    .getattr("points")
                    .unwrap()
                    .extract::<i64>()
                    .unwrap(),
                42
            );
            assert_eq!(
                decoded
                    .bind(py)
                    .getattr("note")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "exact"
            );
        });
    }

    #[test]
    fn ordered_single_writes_reject_invalid_whole_creates_before_execution_target_resolution() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let tag_id = package
                .type_by_label("tag", TypeKind::Attribute)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let tag_class = package
                .class(&tag_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let duplicate = tag_class.call1(("duplicate",)).unwrap();
            let tags = PyTuple::new(py, [duplicate.clone(), duplicate]).unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("tag", tags).unwrap();
            let invalid = person_class.call((), Some(&kwargs)).unwrap();
            invalid
                .call_method1("attach_runtime_iid", ("0xa",))
                .unwrap();
            let manager = PyProjectedModelManager {
                package,
                type_id: person_id,
                database: None,
                transaction: None,
                successor_batch_marker: None,
                runtime: Arc::new(
                    ProviderRuntimeOwner::new().expect("provider runtime should start"),
                ),
                filters: Vec::new(),
                compatibility_filter: None,
            };

            let errors = [
                manager.insert(py, invalid.clone()).unwrap_err(),
                manager.put(py, invalid.clone()).unwrap_err(),
                manager.update(py, invalid).unwrap_err(),
            ];
            for error in errors {
                assert!(!error.to_string().contains("no execution target"));
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "invalid_input"
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    "ordered_distinct_duplicate"
                );
            }
        });
    }

    #[test]
    fn ordered_hydration_rejects_duplicate_scalars_and_players_with_integrity_paths() {
        use pythonize::depythonize;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let person_model = &package.projection.projection().models()[&person_id];
            let tag = person_model.complete_read().fields()[0].token().clone();
            let participant = package.projection.projection().models()[&gathering_id]
                .complete_read()
                .roles()
                .keys()
                .next()
                .unwrap()
                .clone();

            let scalar_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        ("tag".into(), AttributeValue::String("same".into())),
                        ("tag".into(), AttributeValue::String("same".into())),
                    ],
                },
            )
            .unwrap_err();
            let scalar = scalar_error.value(py);
            assert_eq!(
                scalar
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                scalar.getattr("code").unwrap().extract::<String>().unwrap(),
                "ordered_distinct_duplicate"
            );
            let scalar_path: serde_json::Value =
                depythonize(&scalar.getattr("path").unwrap()).unwrap();
            assert_eq!(
                scalar_path,
                serde_json::json!([
                    {"kind": "type", "value": person_id},
                    {"kind": "field", "value": tag},
                    {"kind": "index", "value": 1},
                ])
            );

            let duplicate_player = || DynamicRolePlayer {
                role_name: "participant".into(),
                player_iid: Some("0xa1".into()),
                player_type_name: Some("person".into()),
                attributes: vec![],
            };
            let role_error = hydrate_relation(
                py,
                package.as_ref(),
                &gathering_id,
                &DynamicRelationRow {
                    iid: Some("0xb1".into()),
                    type_name: Some("gathering".into()),
                    attributes: vec![],
                    role_players: vec![duplicate_player(), duplicate_player()],
                },
            )
            .unwrap_err();
            let role = role_error.value(py);
            assert_eq!(
                role.getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                role.getattr("code").unwrap().extract::<String>().unwrap(),
                "ordered_distinct_duplicate"
            );
            let role_path: serde_json::Value = depythonize(&role.getattr("path").unwrap()).unwrap();
            assert_eq!(
                role_path,
                serde_json::json!([
                    {"kind": "type", "value": gathering_id},
                    {"kind": "role", "value": participant},
                    {"kind": "index", "value": 1},
                    {"kind": "type", "value": person_id},
                ])
            );
        });
    }

    #[test]
    fn ordered_hydration_preserves_inherited_field_role_and_reference_order() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let person = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        ("tag".into(), AttributeValue::String("first".into())),
                        ("tag".into(), AttributeValue::String("second".into())),
                    ],
                },
            )
            .expect("a valid inherited ordered ownership hydrates");
            let values = person.bind(py).call_method0("runtime_values").unwrap();
            let tags = values
                .cast::<PyDict>()
                .unwrap()
                .get_item("tag")
                .unwrap()
                .unwrap();
            let tags = tags.cast::<PyTuple>().unwrap();
            let hydrated_tags = tags
                .iter()
                .map(|tag| {
                    tag.call_method0("runtime_attribute_value")
                        .unwrap()
                        .extract::<String>()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(hydrated_tags, ["first", "second"]);

            let player = |iid: &str| DynamicRolePlayer {
                role_name: "participant".into(),
                player_iid: Some(iid.into()),
                player_type_name: Some("person".into()),
                attributes: vec![],
            };
            let gathering = hydrate_relation(
                py,
                package.as_ref(),
                &gathering_id,
                &DynamicRelationRow {
                    iid: Some("0xb1".into()),
                    type_name: Some("gathering".into()),
                    attributes: vec![],
                    role_players: vec![player("0xa1"), player("0xa2")],
                },
            )
            .expect("a valid inherited ordered role hydrates");
            let values = gathering.bind(py).call_method0("runtime_values").unwrap();
            let participants = values
                .cast::<PyDict>()
                .unwrap()
                .get_item("participant")
                .unwrap()
                .unwrap();
            let participants = participants.cast::<PyTuple>().unwrap();
            let hydrated_iids = participants
                .iter()
                .map(|participant| {
                    participant
                        .getattr("iid")
                        .unwrap()
                        .extract::<String>()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(hydrated_iids, ["0xa1", "0xa2"]);
        });
    }

    #[test]
    fn ordered_hydration_maps_scalar_constraints_and_iids_to_integrity() {
        use pythonize::depythonize;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let tag_id = package
                .type_by_label("tag", TypeKind::Attribute)
                .unwrap()
                .clone();

            let scalar_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![("tag".into(), AttributeValue::String(String::new()))],
                },
            )
            .unwrap_err();
            let scalar = scalar_error.value(py);
            assert_eq!(
                scalar
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                scalar.getattr("code").unwrap().extract::<String>().unwrap(),
                "regex_constraint_violation"
            );
            let path: serde_json::Value = depythonize(&scalar.getattr("path").unwrap()).unwrap();
            assert_eq!(path, serde_json::json!([{"kind": "type", "value": tag_id}]));

            let iid_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("not-a-canonical-iid".into()),
                    type_name: Some("person".into()),
                    attributes: vec![],
                },
            )
            .unwrap_err();
            let iid = iid_error.value(py);
            assert_eq!(
                iid.getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                iid.getattr("code").unwrap().extract::<String>().unwrap(),
                "noncanonical_hydrated_iid"
            );
        });
    }

    #[test]
    fn unordered_hydration_keeps_duplicate_members_on_the_legacy_path() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install(py);
            assert!(
                !PyRuntimeProjection {
                    package: Arc::clone(&package),
                }
                .match_session()
                .unwrap()
                .projected_companion_enabled(),
                "legacy unordered queries must keep the released hydration path",
            );
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let hydrated = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0x-person".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        (
                            "identifier".into(),
                            AttributeValue::String("person-1".into()),
                        ),
                        ("aliases".into(), AttributeValue::String("same".into())),
                        ("aliases".into(), AttributeValue::String("same".into())),
                    ],
                },
            )
            .expect("legacy unordered hydration accepts repeated collection members");
            let values = hydrated.bind(py).call_method0("runtime_values").unwrap();
            let aliases = values
                .cast::<PyDict>()
                .unwrap()
                .get_item("aliases")
                .unwrap()
                .unwrap();
            assert_eq!(aliases.cast::<PyTuple>().unwrap().len(), 2);
        });
    }

    #[test]
    fn canonical_snapshot_role_player_rejects_mutation_before_provider_io() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let complete_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let provider_runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let (source_backend, _) =
                OriginRecordingBackend::new(vec![QueryResult::Documents(vec![
                    origin_person_document("0xabc"),
                ])]);
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(Arc::new(Database::with_backend(
                    Box::new(source_backend),
                    "detached-snapshot-source",
                ))),
                None,
                Arc::clone(&provider_runtime),
            );
            let snapshot = source_manager.get_by_iid(py, "0xabc").unwrap();
            let runtime_projection = PyRuntimeProjection {
                package: Arc::clone(&package),
            };
            let snapshot_bytes = runtime_projection
                .encode_snapshot(py, complete_class.clone_ref(py), snapshot.bind(py).clone())
                .unwrap();
            let detached = runtime_projection
                .decode_snapshot(py, complete_class, &snapshot_bytes)
                .unwrap();

            let (backend, state) = OriginRecordingBackend::new(vec![]);
            let manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(Arc::new(Database::with_backend(
                    Box::new(backend),
                    "detached-snapshot-target",
                ))),
                None,
                provider_runtime,
            );
            let gathering =
                gathering_with_player(py, package.as_ref(), &gathering_id, detached.bind(py));
            let error = manager.insert(py, gathering).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "projected_snapshot_detached"
            );
            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn ordered_match_sessions_enable_successor_projected_companions() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            assert!(
                PyRuntimeProjection { package }
                    .match_session()
                    .unwrap()
                    .projected_companion_enabled(),
            );
        });
    }

    #[test]
    fn facade_origin_registry_is_exact_identity_weak_and_eagerly_collected() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let values = PyDict::new(py);
            let original =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa1").unwrap();
            let reference = Arc::new(
                ProjectedReference::try_new(
                    package.projection.as_ref(),
                    person_id,
                    Some("0xa1".into()),
                    vec![],
                )
                .unwrap(),
            );
            package
                .facade_origins
                .retain_reference(original.bind(py), Arc::clone(&reference))
                .unwrap();
            assert!(matches!(
                package.facade_origins.proof(original.bind(py)),
                Some(FacadeProjectionProof::Reference(proof)) if proof.as_ref() == reference.as_ref()
            ));

            let copied = py
                .import("copy")
                .unwrap()
                .call_method1("copy", (original.bind(py),))
                .unwrap();
            let lookalike =
                hydrate_reference(py, package.as_ref(), reference.type_id(), &values, "0xa1")
                    .unwrap();
            assert!(package.facade_origins.proof(&copied).is_none());
            assert!(package.facade_origins.proof(lookalike.bind(py)).is_none());

            let observer = PyWeakrefReference::new(original.bind(py)).unwrap().unbind();
            assert_eq!(package.facade_origins.len(), 1);
            drop(original);
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert!(observer.bind(py).upgrade().is_none());
            assert_eq!(package.facade_origins.len(), 0);
        });
    }

    #[test]
    fn facade_origin_pending_lookup_activation_abort_and_shared_callback_are_atomic() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let reference = Arc::new(
                ProjectedReference::try_new(
                    package.projection.as_ref(),
                    person_id.clone(),
                    Some("0xa1".into()),
                    vec![],
                )
                .unwrap(),
            );
            let values = PyDict::new(py);

            let activated =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa1").unwrap();
            let activation = package
                .facade_origins
                .stage(
                    py,
                    package.facade_origins.prepare(activated.bind(py)).unwrap(),
                    FacadeProjectionProof::Reference(Arc::clone(&reference)),
                    None,
                )
                .unwrap();
            assert!(package.facade_origins.proof(activated.bind(py)).is_none());
            assert_eq!(package.facade_origins.len(), 0);
            package.facade_origins.activate(&[activation]);
            assert!(matches!(
                package.facade_origins.proof(activated.bind(py)),
                Some(FacadeProjectionProof::Reference(proof))
                    if Arc::ptr_eq(&proof, &reference)
            ));

            let live_manual = PyWeakrefReference::new(activated.bind(py)).unwrap();
            package
                .facade_origins
                .callback
                .bind(py)
                .call1((live_manual,))
                .unwrap();
            assert!(package.facade_origins.proof(activated.bind(py)).is_some());

            // Exercise the dead-target callback branch without depending on
            // allocator pointer reuse. Install the old shared weakref under
            // the replacement facade's pointer, expire it, then install the
            // live replacement under that same synthetic key. Replaying the
            // retained stale callback must compare weakref identity and leave
            // the replacement proof intact.
            let expired =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa4").unwrap();
            let expired_origin = package.facade_origins.prepare(expired.bind(py)).unwrap();
            let expired_weakref = expired_origin.facade.clone_ref(py);
            let replacement =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa5").unwrap();
            let replacement_origin = package
                .facade_origins
                .prepare(replacement.bind(py))
                .unwrap();
            let synthetic_key = replacement.bind(py).as_ptr() as usize;
            assert_eq!(replacement_origin.pointer, synthetic_key);
            {
                let mut entries = package
                    .facade_origins
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                assert!(
                    entries
                        .insert(
                            synthetic_key,
                            FacadeOriginEntry {
                                facade: expired_origin.facade,
                                active: Some(FacadeProjectionProof::Reference(Arc::clone(
                                    &reference,
                                ))),
                                pending: None,
                                retired: None,
                            },
                        )
                        .is_none(),
                );
            }
            drop(expired);
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert!(expired_weakref.bind(py).upgrade().is_none());
            assert!(
                package
                    .facade_origins
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&synthetic_key)
                    .is_none(),
            );
            {
                let mut entries = package
                    .facade_origins
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                assert!(
                    entries
                        .insert(
                            synthetic_key,
                            FacadeOriginEntry {
                                facade: replacement_origin.facade,
                                active: Some(FacadeProjectionProof::Reference(Arc::clone(
                                    &reference,
                                ))),
                                pending: None,
                                retired: None,
                            },
                        )
                        .is_none(),
                );
            }
            package
                .facade_origins
                .callback
                .bind(py)
                .call1((expired_weakref.bind(py),))
                .unwrap();
            assert!(matches!(
                package.facade_origins.proof(replacement.bind(py)),
                Some(FacadeProjectionProof::Reference(proof))
                    if Arc::ptr_eq(&proof, &reference)
            ));

            let aborted =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa2").unwrap();
            let aborted_activation = package
                .facade_origins
                .stage(
                    py,
                    package.facade_origins.prepare(aborted.bind(py)).unwrap(),
                    FacadeProjectionProof::Reference(Arc::clone(&reference)),
                    None,
                )
                .unwrap();
            assert!(package.facade_origins.proof(aborted.bind(py)).is_none());
            package.facade_origins.abort(&[aborted_activation]);
            assert!(package.facade_origins.proof(aborted.bind(py)).is_none());

            let retry = package
                .facade_origins
                .stage(
                    py,
                    package.facade_origins.prepare(aborted.bind(py)).unwrap(),
                    FacadeProjectionProof::Reference(Arc::clone(&reference)),
                    None,
                )
                .unwrap();
            assert!(package.facade_origins.proof(aborted.bind(py)).is_none());
            package.facade_origins.activate(&[retry]);
            assert!(package.facade_origins.proof(aborted.bind(py)).is_some());

            let helper = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
class ReenterOnDrop:
    def __init__(self, callback, weakref, seen):
        self.callback = callback
        self.weakref = weakref
        self.seen = seen
    def __del__(self):
        self.seen.append("dropped")
        self.callback(self.weakref)
"#
                ),
                ffi::c_str!("python_origin_drop.py"),
                ffi::c_str!("python_origin_drop"),
            )
            .unwrap();
            let retired_facade =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa3").unwrap();
            let unrelated_weakref = PyWeakrefReference::new(retired_facade.bind(py)).unwrap();
            let seen = PyList::empty(py);
            let finalizer = helper
                .getattr("ReenterOnDrop")
                .unwrap()
                .call1((
                    package.facade_origins.callback.bind(py),
                    unrelated_weakref,
                    &seen,
                ))
                .unwrap()
                .unbind();
            let retired_activation = package
                .facade_origins
                .stage(
                    py,
                    package
                        .facade_origins
                        .prepare(retired_facade.bind(py))
                        .unwrap(),
                    FacadeProjectionProof::Reference(reference),
                    Some(ProjectedFacadeSnapshot {
                        iid: py.None(),
                        values: finalizer,
                    }),
                )
                .unwrap();
            assert!(
                package
                    .facade_origins
                    .proof(retired_facade.bind(py))
                    .is_none()
            );
            package.facade_origins.abort(&[retired_activation]);
            assert_eq!(seen.len(), 1);
            assert_eq!(
                seen.get_item(0).unwrap().extract::<String>().unwrap(),
                "dropped"
            );
            assert!(
                package
                    .facade_origins
                    .proof(retired_facade.bind(py))
                    .is_none()
            );

            drop(activated);
            drop(aborted);
            drop(replacement);
            drop(retired_facade);
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert_eq!(package.facade_origins.len(), 0);
        });
    }

    #[test]
    fn hydrated_facade_origin_is_absent_from_python_introspection_and_serialization() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let (backend, _) = OriginRecordingBackend::new(vec![QueryResult::Documents(vec![
                origin_person_document("0xa0"),
            ])]);
            let database = Arc::new(Database::with_backend(
                Box::new(backend),
                "private-origin-secret",
            ));
            let manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(database),
                None,
                runtime,
            );
            let facade = manager
                .get_by_iid(py, "0xa0")
                .expect("ordered manager hydration should succeed");
            assert!(package.facade_origins.proof(facade.bind(py)).is_some());

            let copied = py
                .import("copy")
                .unwrap()
                .call_method1("copy", (facade.bind(py),))
                .unwrap();
            assert!(package.facade_origins.proof(&copied).is_none());

            let builtins = py.import("builtins").unwrap();
            let names = builtins
                .call_method1("dir", (facade.bind(py),))
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap();
            let forbidden = [
                "origin",
                "origin_carrier",
                "origin_token",
                "database_identity",
                "authority",
            ];
            assert!(names.iter().all(|name| {
                let name = name.to_ascii_lowercase();
                forbidden.iter().all(|needle| !name.contains(needle))
            }));
            for name in forbidden {
                assert!(!facade.bind(py).hasattr(name).unwrap());
            }

            let vars_error = builtins
                .call_method1("vars", (facade.bind(py),))
                .unwrap_err();
            assert!(vars_error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            let visible = PyDict::new(py);
            visible
                .set_item("_iid", facade.bind(py).getattr("iid").unwrap())
                .unwrap();
            visible
                .set_item(
                    "_values",
                    facade.bind(py).call_method0("runtime_values").unwrap(),
                )
                .unwrap();
            let mut keys = visible
                .keys()
                .iter()
                .map(|key| key.extract::<String>().unwrap())
                .collect::<Vec<_>>();
            keys.sort();
            assert_eq!(keys, ["_iid", "_values"]);

            let json = py.import("json").unwrap();
            let public_adapter = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
def public_state(value):
    if hasattr(value, "runtime_attribute_value"):
        return value.runtime_attribute_value()
    return {"_iid": value.iid, "_values": value.runtime_values()}
"#
                ),
                ffi::c_str!("python_public_projection_state.py"),
                ffi::c_str!("python_public_projection_state"),
            )
            .unwrap();
            let options = PyDict::new(py);
            options
                .set_item("default", public_adapter.getattr("public_state").unwrap())
                .unwrap();
            options.set_item("sort_keys", true).unwrap();
            let serialized = json
                .call_method("dumps", (facade.bind(py),), Some(&options))
                .unwrap()
                .extract::<String>()
                .unwrap();
            let public_type = facade
                .bind(py)
                .get_type()
                .repr()
                .unwrap()
                .extract::<String>()
                .unwrap();
            let public_mro = facade
                .bind(py)
                .get_type()
                .getattr("__mro__")
                .unwrap()
                .repr()
                .unwrap()
                .extract::<String>()
                .unwrap();
            let representation = facade.bind(py).repr().unwrap().extract::<String>().unwrap();
            for public_text in [serialized, public_type, public_mro, representation] {
                let public_text = public_text.to_ascii_lowercase();
                assert!(!public_text.contains("private-origin-secret"));
                assert!(forbidden.iter().all(|needle| !public_text.contains(needle)));
            }

            let operator = py.import("operator").unwrap();
            assert!(
                operator
                    .call_method1("eq", (facade.bind(py), facade.bind(py)))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            assert!(
                !operator
                    .call_method1("eq", (facade.bind(py), &copied))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
        });
    }

    #[test]
    fn ordered_manager_facades_accept_same_origin_and_fence_foreign_before_io() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let authority = DatabaseConnectionAuthority::isolated();
            let (source_backend, source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xa1")]),
            ]);
            let source = Arc::new(Database::with_backend_authority(
                Box::new(source_backend),
                "shared",
                authority.clone(),
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(source),
                None,
                Arc::clone(&runtime),
            );
            let person = source_manager
                .get_by_iid(py, "0xa1")
                .expect("ordered manager hydration should succeed");
            assert!(package.facade_origins.proof(person.bind(py)).is_some());
            let registry_weakref = py
                .import("weakref")
                .unwrap()
                .call_method1("getweakrefs", (person.bind(py),))
                .unwrap()
                .cast_into::<PyList>()
                .unwrap()
                .iter()
                .find(|candidate| {
                    candidate
                        .getattr("__callback__")
                        .is_ok_and(|callback| !callback.is_none())
                })
                .expect("the native registry weakref has a cleanup callback");
            registry_weakref
                .getattr("__callback__")
                .unwrap()
                .call1((&registry_weakref,))
                .expect("a hostile live cleanup callback call remains harmless");
            assert!(
                package.facade_origins.proof(person.bind(py)).is_some(),
                "a live caller cannot erase opaque origin proof by invoking the weakref callback",
            );

            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb1"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb1", "0xa1")]),
            ]);
            let same = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same),
                None,
                Arc::clone(&runtime),
            );
            let same_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            same_manager
                .insert(py, same_value)
                .expect("same database authority should accept its hydrated facade");
            {
                let state = same_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (foreign_backend, foreign_state) = OriginRecordingBackend::new(vec![]);
            let foreign = Arc::new(Database::with_backend(Box::new(foreign_backend), "shared"));
            let foreign_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(foreign),
                None,
                runtime,
            );
            let foreign_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            let error = foreign_manager.insert(py, foreign_value).unwrap_err();
            let diagnostic = error.value(py);
            assert_eq!(
                diagnostic
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "invalid_input"
            );
            assert_eq!(
                diagnostic
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            let state = foreign_state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
            assert_eq!(source_state.lock().unwrap().closes, 1);
        });
    }

    #[test]
    fn ordered_borrowed_facade_keeps_transaction_authority_identity() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let authority = DatabaseConnectionAuthority::isolated();
            let (source_backend, source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xa2")]),
            ]);
            let source = Arc::new(Database::with_backend_authority(
                Box::new(source_backend),
                "shared",
                authority.clone(),
            ));
            let transaction = provider_block_on(
                py,
                runtime.as_ref(),
                source.transaction_context(TxType::Read),
            )
            .unwrap();
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                None,
                Some(transaction.clone()),
                Arc::clone(&runtime),
            );
            let person = source_manager
                .get_by_iid(py, "0xa2")
                .expect("borrowed transaction hydration should succeed");

            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb2"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb2", "0xa2")]),
            ]);
            let same = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same),
                None,
                Arc::clone(&runtime),
            );
            let value = gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            same_manager
                .insert(py, value)
                .expect("borrowed proof should share its database authority");
            assert_eq!(same_state.lock().unwrap().commits, 1);

            provider_block_on(py, runtime.as_ref(), transaction.close()).unwrap();
            let state = source_state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Read]);
            assert_eq!(state.queries.len(), 1);
            assert_eq!(state.closes, 1);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn ordered_query_facades_bind_direct_and_borrowed_origins_but_remote_stays_unbound() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));

            let (direct_backend, direct_state) = QueryOriginBackend::new("0xa3");
            let direct_database = Arc::new(Database::with_backend(
                Box::new(direct_backend),
                "query-direct",
            ));
            let (direct_registry, direct_request) = person_query(package.as_ref());
            let direct_result = provider_block_on(
                py,
                runtime.as_ref(),
                direct_database.execute_match(&direct_registry, &direct_request),
            )
            .unwrap();
            let direct_handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::for_database(direct_database.as_ref())),
                direct_request,
                direct_result,
                direct_registry,
                query_budget(),
            )
            .unwrap();
            let direct = hydrate_first_query_thing(py, package.as_ref(), direct_handle);
            let direct_proof = package.facade_origins.proof(direct.bind(py)).unwrap();
            assert!(
                direct_proof
                    .reference(package.projection.as_ref())
                    .unwrap()
                    .origin_carrier()
                    .is_some()
            );

            let (foreign_backend, foreign_state) = OriginRecordingBackend::new(vec![]);
            let foreign = Arc::new(Database::with_backend(
                Box::new(foreign_backend),
                "query-foreign",
            ));
            let foreign_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(foreign),
                None,
                Arc::clone(&runtime),
            );
            let foreign_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, direct.bind(py));
            let error = foreign_manager.insert(py, foreign_value).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = foreign_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = direct_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }

            let authority = DatabaseConnectionAuthority::isolated();
            let (borrowed_backend, borrowed_state) = QueryOriginBackend::new("0xa4");
            let borrowed_database = Arc::new(Database::with_backend_authority(
                Box::new(borrowed_backend),
                "query-shared",
                authority.clone(),
            ));
            let borrowed_transaction = provider_block_on(
                py,
                runtime.as_ref(),
                borrowed_database.transaction_context(TxType::Read),
            )
            .unwrap();
            let (borrowed_registry, borrowed_request) = person_query(package.as_ref());
            let borrowed_result = provider_block_on(
                py,
                runtime.as_ref(),
                borrowed_transaction.execute_match(&borrowed_registry, &borrowed_request),
            )
            .unwrap();
            let borrowed_origin =
                ProjectedQueryOrigin::for_transaction(&borrowed_transaction).unwrap();
            let borrowed_handle = validated_result_handle(
                Some(&package.projection),
                Some(borrowed_origin),
                borrowed_request,
                borrowed_result,
                borrowed_registry,
                query_budget(),
            )
            .unwrap();
            let borrowed = hydrate_first_query_thing(py, package.as_ref(), borrowed_handle);
            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb4"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb4", "0xa4")]),
            ]);
            let same_database = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "query-shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same_database),
                None,
                Arc::clone(&runtime),
            );
            let same_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, borrowed.bind(py));
            same_manager
                .insert(py, same_value)
                .expect("borrowed query facade should keep database authority identity");
            assert_eq!(same_state.lock().unwrap().commits, 1);
            provider_block_on(py, runtime.as_ref(), borrowed_transaction.close()).unwrap();
            {
                let state = borrowed_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }

            let (remote_backend, remote_state) = QueryOriginBackend::new("0xa5");
            let remote_evidence_database = Arc::new(Database::with_backend(
                Box::new(remote_backend),
                "remote-evidence",
            ));
            let (remote_registry, remote_request) = person_query(package.as_ref());
            let remote_result = provider_block_on(
                py,
                runtime.as_ref(),
                remote_evidence_database.execute_match(&remote_registry, &remote_request),
            )
            .unwrap();
            let remote_handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::remote_unbound()),
                remote_request,
                remote_result,
                remote_registry,
                query_budget(),
            )
            .unwrap();
            let remote = hydrate_first_query_thing(py, package.as_ref(), remote_handle);
            let remote_reference = || {
                package
                    .facade_origins
                    .proof(remote.bind(py))
                    .unwrap()
                    .reference(package.projection.as_ref())
                    .unwrap()
            };
            assert!(remote_reference().origin_carrier().is_none());

            let (target_backend, target_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb5"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb5", "0xa5")]),
            ]);
            let target = Arc::new(Database::with_backend(
                Box::new(target_backend),
                "remote-target",
            ));
            let target_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(target),
                None,
                runtime,
            );
            let target_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, remote.bind(py));
            target_manager
                .insert(py, target_value)
                .expect("remote query facade must remain explicitly unbound and resolvable");
            assert!(remote_reference().origin_carrier().is_none());
            {
                let state = target_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = remote_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }
        });
    }

    #[test]
    fn query_hydration_rejects_a_mismatched_raw_and_projected_companion() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let runtime = ProviderRuntimeOwner::new().expect("provider runtime should start");

            let (raw_backend, _) = QueryOriginBackend::new("0xae");
            let raw_database = Database::with_backend(Box::new(raw_backend), "mismatch-raw");
            let (raw_registry, raw_request) = person_query(package.as_ref());
            let raw_result = provider_block_on(
                py,
                &runtime,
                raw_database.execute_match(&raw_registry, &raw_request),
            )
            .unwrap();

            let (projected_backend, _) = QueryOriginBackend::new("0xaf");
            let projected_database =
                Database::with_backend(Box::new(projected_backend), "mismatch-projected");
            let (projected_registry, projected_request) = person_query(package.as_ref());
            let projected_result = provider_block_on(
                py,
                &runtime,
                projected_database.execute_match(&projected_registry, &projected_request),
            )
            .unwrap();
            let projected = ProjectedQueryOrigin::remote_unbound()
                .materialize_borrowed_with_budget(
                    package.projection.as_ref(),
                    &projected_registry,
                    &projected_request,
                    &projected_result,
                    ProjectedQueryMaterializationLimits::default(),
                    &AnswerCancellation::default(),
                    None,
                )
                .unwrap()
                .0;

            let handle = crate::validated_result_runtime::PyValidatedMatchResultHandle::new_with_projected_budget(
                raw_request,
                raw_result,
                raw_registry,
                projected,
                QueryExecutionDeadline::for_limits(QueryExecutionResourceLimits::default()),
                AnswerCancellation::default(),
            );
            let handle = Py::new(py, handle).unwrap();
            let row = handle.bind(py).call_method1("row", (0,)).unwrap();
            let slot = row.call_method1("slot", (0,)).unwrap();
            let thing = slot.call_method1("thing", (0,)).unwrap();
            let thing = thing
                .extract::<PyRef<'_, PyValidatedMatchThingHandle>>()
                .unwrap();
            let error = hydrate_validated_thing(py, package.as_ref(), &thing).unwrap_err();
            let diagnostic = error.value(py);
            assert_eq!(
                diagnostic
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                diagnostic
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "projected_query_result_mismatch"
            );
        });
    }

    #[test]
    fn ordered_manager_and_query_relation_players_fence_foreign_origin_before_io() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let container_id = package
                .type_by_label("container", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let manager_authority = DatabaseConnectionAuthority::isolated();

            let (manager_backend, manager_source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_gathering_document("0xb6", "0xa6")]),
            ]);
            let manager_source = Arc::new(Database::with_backend_authority(
                Box::new(manager_backend),
                "manager-relation-shared",
                manager_authority.clone(),
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(manager_source),
                None,
                Arc::clone(&runtime),
            );
            let manager_relation = source_manager
                .get_by_iid(py, "0xb6")
                .expect("manager relation hydration should succeed");

            let (manager_same_backend, manager_same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb6"})]),
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xc6"})]),
                QueryResult::Documents(vec![origin_container_document("0xc6", "0xb6")]),
            ]);
            let manager_same = Arc::new(Database::with_backend_authority(
                Box::new(manager_same_backend),
                "manager-relation-shared",
                manager_authority,
            ));
            let manager_same_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(manager_same),
                None,
                Arc::clone(&runtime),
            );
            let manager_same_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                manager_relation.bind(py),
            );
            manager_same_manager
                .insert(py, manager_same_container)
                .expect("same authority must accept a hydrated relation-as-player reference");
            {
                let state = manager_same_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 3);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (manager_target_backend, manager_target_state) =
                OriginRecordingBackend::new(vec![]);
            let manager_target = Arc::new(Database::with_backend(
                Box::new(manager_target_backend),
                "manager-relation-target",
            ));
            let manager_target_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(manager_target),
                None,
                Arc::clone(&runtime),
            );
            let manager_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                manager_relation.bind(py),
            );
            let error = manager_target_manager
                .insert(py, manager_container)
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = manager_target_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            assert_eq!(manager_source_state.lock().unwrap().closes, 1);

            let (query_backend, query_source_state) = QueryOriginBackend::gathering("0xb7");
            let query_source = Arc::new(Database::with_backend(
                Box::new(query_backend),
                "query-relation-source",
            ));
            let (registry, request) = thing_query(package.as_ref(), "gathering");
            let result = provider_block_on(
                py,
                runtime.as_ref(),
                query_source.execute_match(&registry, &request),
            )
            .unwrap();
            let handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::for_database(query_source.as_ref())),
                request,
                result,
                registry,
                query_budget(),
            )
            .unwrap();
            let query_relation = hydrate_first_query_thing(py, package.as_ref(), handle);
            let (query_target_backend, query_target_state) = OriginRecordingBackend::new(vec![]);
            let query_target = Arc::new(Database::with_backend(
                Box::new(query_target_backend),
                "query-relation-target",
            ));
            let query_target_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(query_target),
                None,
                runtime,
            );
            let query_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                query_relation.bind(py),
            );
            let error = query_target_manager
                .insert(py, query_container)
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = query_target_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = query_source_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }
        });
    }

    #[test]
    fn retained_facade_mutation_is_integrity_failure_before_target_io() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let (source_backend, _) =
                OriginRecordingBackend::new(vec![QueryResult::Documents(vec![
                    origin_person_document("0xa8"),
                ])]);
            let source = Arc::new(Database::with_backend(
                Box::new(source_backend),
                "mutation-source",
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(source),
                None,
                Arc::clone(&runtime),
            );
            let person = source_manager.get_by_iid(py, "0xa8").unwrap();
            person.bind(py).setattr("_iid", "0xa9").unwrap();

            let (target_backend, target_state) = OriginRecordingBackend::new(vec![]);
            let target = Arc::new(Database::with_backend(
                Box::new(target_backend),
                "mutation-target",
            ));
            let target_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(target),
                None,
                runtime,
            );
            let value = gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            let error = target_manager.insert(py, value).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "hydrated_facade_evidence_mismatch"
            );
            let state = target_state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn retained_reference_key_mutation_is_not_silently_rebound() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let model = &package.projection.projection().models()[&person_id];
            let key_id = model.reference_read().key_fields()[0].clone();
            let attribute_id = package
                .type_by_label(key_id.attribute().label().as_str(), TypeKind::Attribute)
                .unwrap()
                .clone();
            let key = ProjectedAttributeValue::try_from_attribute_value(
                package.projection.as_ref(),
                attribute_id.clone(),
                &AttributeValue::String("original-key".into()),
            )
            .unwrap();
            let reference = Arc::new(
                ProjectedReference::try_new(
                    package.projection.as_ref(),
                    person_id.clone(),
                    Some("0xaa".into()),
                    vec![(key_id.clone(), key.clone())],
                )
                .unwrap(),
            );
            let fields = BTreeMap::from([(key_id.clone(), key)]);
            let values = hydrate_projected_fields(
                py,
                package.as_ref(),
                &person_id,
                HydratedProjectedFields::Reference(&fields),
                false,
                None,
            )
            .unwrap();
            let facade =
                allocate_projected_reference(py, package.as_ref(), &person_id, &values, "0xaa")
                    .unwrap();
            package
                .facade_origins
                .retain_reference(facade.bind(py), reference)
                .unwrap();

            let changed = package
                .class(&attribute_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call1(("changed-key",))
                .unwrap();
            facade
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .cast::<PyDict>()
                .unwrap()
                .set_item(
                    model.query_tokens().fields()[&key_id]
                        .target_name()
                        .as_str(),
                    changed,
                )
                .unwrap();
            let allowed = BTreeSet::from([ProjectedModelUse::new(
                person_id,
                ProjectedModelForm::Reference,
            )]);
            let error = project_reference(py, package.as_ref(), facade.bind(py), &allowed, &[])
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "hydrated_facade_evidence_mismatch"
            );
        });
    }

    #[test]
    fn ordered_insert_and_put_publish_exact_provider_rehydration() {
        Python::initialize();
        Python::attach(|py| {
            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let tag_id = package
                    .type_by_label("tag", TypeKind::Attribute)
                    .unwrap()
                    .clone();
                let iid = if put { "0xab" } else { "0xaa" };
                let responses = vec![
                    QueryResult::Documents(vec![serde_json::json!({"iid": iid})]),
                    QueryResult::Documents(vec![serde_json::json!({
                        "_iid": iid,
                        "_type": "person",
                        "attributes": {
                            "tag": [{"value": "provider-normalized"}]
                        }
                    })]),
                ];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(Arc::new(Database::with_backend(
                        Box::new(backend),
                        if put {
                            "provider-normalized-put"
                        } else {
                            "provider-normalized-insert"
                        },
                    ))),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let authored = package
                    .class(&tag_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call1(("authored",))
                    .unwrap();
                let kwargs = PyDict::new(py);
                kwargs
                    .set_item("tag", PyTuple::new(py, [authored]).unwrap())
                    .unwrap();
                let instance = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call((), Some(&kwargs))
                    .unwrap();

                let published = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .expect("valid provider rehydration should publish");
                assert!(published.bind(py).is(&instance));
                assert_eq!(
                    instance
                        .getattr("iid")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    iid
                );
                let values = instance
                    .call_method0("runtime_values")
                    .unwrap()
                    .cast_into::<PyDict>()
                    .unwrap();
                let tags = values
                    .get_item("tag")
                    .unwrap()
                    .unwrap()
                    .cast_into::<PyTuple>()
                    .unwrap();
                assert_eq!(tags.len(), 1);
                assert_eq!(
                    tags.get_item(0)
                        .unwrap()
                        .call_method0("runtime_attribute_value")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "provider-normalized"
                );

                let proof = match package.facade_origins.proof(&instance).unwrap() {
                    FacadeProjectionProof::Thing(proof) => proof,
                    FacadeProjectionProof::Reference(_) => {
                        panic!("insert/put retained a reference proof")
                    }
                    FacadeProjectionProof::RolePlayer { .. } => {
                        panic!("insert/put retained a nested role-player proof")
                    }
                    FacadeProjectionProof::DetachedSnapshot => {
                        panic!("insert/put retained a detached snapshot proof")
                    }
                };
                let field_id = package.projection.projection().models()[&person_id]
                    .complete_read()
                    .fields()[0]
                    .token();
                assert_eq!(
                    proof.fields()[field_id][0].to_attribute_value(),
                    AttributeValue::String("provider-normalized".into())
                );
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }
        });
    }

    #[test]
    fn ordered_insert_and_put_reject_malformed_provider_iids_without_publication() {
        Python::initialize();
        Python::attach(|py| {
            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let responses = vec![QueryResult::Documents(vec![serde_json::json!({
                    "iid": "not-a-canonical-iid"
                })])];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let database = Arc::new(Database::with_backend(
                    Box::new(backend),
                    if put {
                        "malformed-put"
                    } else {
                        "malformed-insert"
                    },
                ));
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(database),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let instance = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call0()
                    .unwrap();
                let error = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .unwrap_err();
                let diagnostic = error.value(py);
                assert_eq!(
                    diagnostic
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity"
                );
                assert_eq!(
                    diagnostic
                        .getattr("code")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "provider_hydration_failed"
                );
                assert!(instance.getattr("iid").unwrap().is_none());
                assert!(package.facade_origins.proof(&instance).is_none());
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 1);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 1);
            }
        });
    }

    #[test]
    fn ordered_insert_put_and_update_restore_exact_facade_state_after_local_failure() {
        Python::initialize();
        Python::attach(|py| {
            let helpers = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
def failing_attach(target):
    def attach(self, iid):
        if self is target:
            self._iid = iid
            self._values = {"corrupt": True}
            raise RuntimeError("injected attach failure")
        self._iid = iid
    return attach

def failing_initialize(target, original):
    def initialize(self, values):
        if self is target:
            self._values = {"corrupt": True}
            self._iid = "0xff"
            raise RuntimeError("injected replacement failure")
        return original(self, values)
    return initialize
"#
                ),
                ffi::c_str!("origin_failure_helpers.py"),
                ffi::c_str!("origin_failure_helpers"),
            )
            .unwrap();

            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let responses = vec![
                    QueryResult::Documents(vec![serde_json::json!({"iid": "0xac"})]),
                    QueryResult::Documents(vec![origin_person_document("0xac")]),
                ];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(Arc::new(Database::with_backend(
                        Box::new(backend),
                        if put {
                            "rollback-put"
                        } else {
                            "rollback-insert"
                        },
                    ))),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let class = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py);
                let instance = class.call0().unwrap();
                let prior_values = instance.getattr("_values").unwrap().unbind();
                let prior_iid = instance.getattr("_iid").unwrap().unbind();
                let original_attach = class.getattr("attach_runtime_iid").unwrap().unbind();
                let failing = helpers
                    .getattr("failing_attach")
                    .unwrap()
                    .call1((&instance,))
                    .unwrap();
                class.setattr("attach_runtime_iid", failing).unwrap();

                let error = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .unwrap_err();
                class
                    .setattr("attach_runtime_iid", original_attach.bind(py))
                    .unwrap();
                assert!(error.to_string().contains("injected attach failure"));
                assert!(
                    instance
                        .getattr("_values")
                        .unwrap()
                        .is(prior_values.bind(py))
                );
                assert!(instance.getattr("_iid").unwrap().is(prior_iid.bind(py)));
                assert!(package.facade_origins.proof(&instance).is_none());
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let (backend, state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xad")]),
                QueryResult::Ok,
                QueryResult::Documents(vec![origin_person_document("0xad")]),
            ]);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(Arc::new(Database::with_backend(
                    Box::new(backend),
                    "rollback-update",
                ))),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let instance = manager.get_by_iid(py, "0xad").unwrap();
            let prior_values = instance.bind(py).getattr("_values").unwrap().unbind();
            let prior_iid = instance.bind(py).getattr("_iid").unwrap().unbind();
            let prior_proof = match package.facade_origins.proof(instance.bind(py)).unwrap() {
                FacadeProjectionProof::Thing(proof) => proof,
                FacadeProjectionProof::Reference(_) => panic!("manager get retained a reference"),
                FacadeProjectionProof::RolePlayer { .. } => {
                    panic!("manager get retained a nested role-player proof")
                }
                FacadeProjectionProof::DetachedSnapshot => {
                    panic!("manager get retained a detached snapshot proof")
                }
            };
            let class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let original_initialize = class.getattr("initialize_runtime_values").unwrap().unbind();
            let failing = helpers
                .getattr("failing_initialize")
                .unwrap()
                .call1((instance.bind(py), original_initialize.bind(py)))
                .unwrap();
            class.setattr("initialize_runtime_values", failing).unwrap();
            let error = manager.update(py, instance.bind(py).clone()).unwrap_err();
            class
                .setattr("initialize_runtime_values", original_initialize.bind(py))
                .unwrap();
            assert!(error.to_string().contains("injected replacement failure"));
            assert!(
                instance
                    .bind(py)
                    .getattr("_values")
                    .unwrap()
                    .is(prior_values.bind(py))
            );
            assert!(
                instance
                    .bind(py)
                    .getattr("_iid")
                    .unwrap()
                    .is(prior_iid.bind(py))
            );
            let after_proof = match package.facade_origins.proof(instance.bind(py)).unwrap() {
                FacadeProjectionProof::Thing(proof) => proof,
                FacadeProjectionProof::Reference(_) => panic!("update retained a reference"),
                FacadeProjectionProof::RolePlayer { .. } => {
                    panic!("update retained a nested role-player proof")
                }
                FacadeProjectionProof::DetachedSnapshot => {
                    panic!("update retained a detached snapshot proof")
                }
            };
            assert!(Arc::ptr_eq(&prior_proof, &after_proof));
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Read, TxType::Write]);
            assert_eq!(state.queries.len(), 3);
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
            assert_eq!(state.closes, 1);
        });
    }

    #[test]
    fn successor_entity_batches_route_all_operations_for_owned_and_borrowed_targets() {
        Python::initialize();
        Python::attach(|py| {
            for borrowed in [false, true] {
                for operation in [
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchOperation::Update,
                    ProjectedBatchOperation::Delete,
                ] {
                    let (_, package) = install_batch(py);
                    let (database, state) =
                        BatchRecordingBackend::fixture(entity_batch_responses(operation), false);
                    let runtime = Arc::new(
                        ProviderRuntimeOwner::new().expect("provider runtime should start"),
                    );
                    let transaction = borrowed.then(|| {
                        runtime
                            .block_on(database.transaction_context(TxType::Write))
                            .unwrap()
                    });
                    let manager = origin_manager(
                        Arc::clone(&package),
                        package
                            .type_by_label("person", TypeKind::Entity)
                            .unwrap()
                            .clone(),
                        (!borrowed).then(|| Arc::clone(&database)),
                        transaction.clone(),
                        Arc::clone(&runtime),
                    );
                    let instances = (0..2)
                        .map(|ordinal| {
                            batch_person(
                                py,
                                package.as_ref(),
                                &format!("person-{ordinal}"),
                                &format!("authored-{ordinal}"),
                                (operation == ProjectedBatchOperation::Update)
                                    .then_some(if ordinal == 0 { "0x20" } else { "0x21" }),
                            )
                        })
                        .collect::<Vec<_>>();
                    let pointers = instances
                        .iter()
                        .map(|instance| instance.bind(py).as_ptr())
                        .collect::<Vec<_>>();
                    if operation == ProjectedBatchOperation::Delete {
                        let iids = PyList::new(py, ["0x20", "0x21"]).unwrap();
                        manager.delete_many(py, iids.into_any()).unwrap();
                    } else {
                        let rows =
                            PyList::new(py, instances.iter().map(|instance| instance.bind(py)))
                                .unwrap()
                                .into_any();
                        let output = match operation {
                            ProjectedBatchOperation::Insert => manager.insert_many(py, rows),
                            ProjectedBatchOperation::Put => manager.put_many(py, rows),
                            ProjectedBatchOperation::Update => manager.update_many(py, rows),
                            ProjectedBatchOperation::Delete => unreachable!(),
                        }
                        .unwrap();
                        let output = output.bind(py).cast::<PyList>().unwrap();
                        assert_eq!(output.len(), 2);
                        for (ordinal, pointer) in pointers.iter().enumerate() {
                            assert_eq!(output.get_item(ordinal).unwrap().as_ptr(), *pointer);
                            assert_eq!(
                                output
                                    .get_item(ordinal)
                                    .unwrap()
                                    .getattr("iid")
                                    .unwrap()
                                    .extract::<String>()
                                    .unwrap(),
                                if ordinal == 0 { "0x20" } else { "0x21" },
                            );
                        }
                    }
                    if let Some(transaction) = transaction {
                        assert_eq!(
                            runtime.block_on(transaction.lifecycle_state()),
                            TransactionContextState::Active,
                        );
                    }
                    let state = state.lock().unwrap();
                    assert!(state.responses.is_empty());
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.rollbacks, 0);
                    assert_eq!(state.legacy_commits, 0);
                    assert_eq!(state.commits, usize::from(!borrowed));
                    let expected_calls = match operation {
                        ProjectedBatchOperation::Put => 3,
                        ProjectedBatchOperation::Delete => 1,
                        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Update => 2,
                    };
                    assert_eq!(state.calls.len(), expected_calls);
                    assert_successor_batch_route_fingerprint(
                        TypeKind::Entity,
                        operation,
                        &state.calls,
                    );
                }
            }
        });
    }

    #[test]
    fn successor_relation_batches_route_all_operations_for_owned_and_borrowed_targets() {
        Python::initialize();
        Python::attach(|py| {
            for borrowed in [false, true] {
                for operation in [
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchOperation::Update,
                    ProjectedBatchOperation::Delete,
                ] {
                    let (_, package) = install_batch(py);
                    let (database, state) =
                        BatchRecordingBackend::fixture(relation_batch_responses(operation), false);
                    let runtime = Arc::new(
                        ProviderRuntimeOwner::new().expect("provider runtime should start"),
                    );
                    let transaction = borrowed.then(|| {
                        runtime
                            .block_on(database.transaction_context(TxType::Write))
                            .unwrap()
                    });
                    let manager = origin_manager(
                        Arc::clone(&package),
                        package
                            .type_by_label("membership", TypeKind::Relation)
                            .unwrap()
                            .clone(),
                        (!borrowed).then(|| Arc::clone(&database)),
                        transaction.clone(),
                        Arc::clone(&runtime),
                    );
                    let players = (0..2)
                        .map(|ordinal| {
                            batch_person(
                                py,
                                package.as_ref(),
                                &format!("player-{ordinal}"),
                                &format!("player-tag-{ordinal}"),
                                None,
                            )
                        })
                        .collect::<Vec<_>>();
                    let instances = players
                        .iter()
                        .enumerate()
                        .map(|(ordinal, player)| {
                            batch_membership(
                                py,
                                package.as_ref(),
                                &format!("membership-{ordinal}"),
                                player,
                                (operation == ProjectedBatchOperation::Update)
                                    .then_some(if ordinal == 0 { "0x30" } else { "0x31" }),
                            )
                        })
                        .collect::<Vec<_>>();
                    let pointers = instances
                        .iter()
                        .map(|instance| instance.bind(py).as_ptr())
                        .collect::<Vec<_>>();
                    if operation == ProjectedBatchOperation::Delete {
                        let iids = PyList::new(py, ["0x30", "0x31"]).unwrap();
                        manager.delete_many(py, iids.into_any()).unwrap();
                    } else {
                        let rows =
                            PyList::new(py, instances.iter().map(|instance| instance.bind(py)))
                                .unwrap()
                                .into_any();
                        let output = match operation {
                            ProjectedBatchOperation::Insert => manager.insert_many(py, rows),
                            ProjectedBatchOperation::Put => manager.put_many(py, rows),
                            ProjectedBatchOperation::Update => manager.update_many(py, rows),
                            ProjectedBatchOperation::Delete => unreachable!(),
                        }
                        .unwrap();
                        let output = output.bind(py).cast::<PyList>().unwrap();
                        assert_eq!(output.len(), 2);
                        for (ordinal, pointer) in pointers.iter().enumerate() {
                            let value = output.get_item(ordinal).unwrap();
                            assert_eq!(value.as_ptr(), *pointer);
                            assert_eq!(
                                value.getattr("iid").unwrap().extract::<String>().unwrap(),
                                if ordinal == 0 { "0x30" } else { "0x31" },
                            );
                        }
                    }
                    if let Some(transaction) = transaction {
                        assert_eq!(
                            runtime.block_on(transaction.lifecycle_state()),
                            TransactionContextState::Active,
                        );
                    }
                    let state = state.lock().unwrap();
                    assert!(state.responses.is_empty());
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.rollbacks, 0);
                    assert_eq!(state.legacy_commits, 0);
                    assert_eq!(state.commits, usize::from(!borrowed));
                    assert_eq!(
                        state.calls.len(),
                        if operation == ProjectedBatchOperation::Delete {
                            1
                        } else {
                            3
                        },
                    );
                    assert_successor_batch_route_fingerprint(
                        TypeKind::Relation,
                        operation,
                        &state.calls,
                    );
                }
            }
        });
    }

    #[test]
    fn successor_empty_batches_validate_without_mapper_or_provider_work() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let unrelated_attribute = package
                .class(
                    package.type_by_label("tag", TypeKind::Attribute).unwrap(),
                    ProjectedModelForm::Complete,
                )
                .unwrap()
                .bind(py);
            unrelated_attribute
                .setattr("_attribute_value", py.None())
                .unwrap();
            for kind in [TypeKind::Entity, TypeKind::Relation] {
                for borrowed in [false, true] {
                    for operation in [
                        ProjectedBatchOperation::Insert,
                        ProjectedBatchOperation::Put,
                        ProjectedBatchOperation::Update,
                        ProjectedBatchOperation::Delete,
                    ] {
                        let (database, state) = BatchRecordingBackend::fixture(vec![], false);
                        let runtime = Arc::new(
                            ProviderRuntimeOwner::new().expect("provider runtime should start"),
                        );
                        let transaction = borrowed.then(|| {
                            runtime
                                .block_on(database.transaction_context(TxType::Write))
                                .unwrap()
                        });
                        let manager = origin_manager(
                            Arc::clone(&package),
                            package
                                .type_by_label(
                                    if kind == TypeKind::Entity {
                                        "person"
                                    } else {
                                        "membership"
                                    },
                                    kind,
                                )
                                .unwrap()
                                .clone(),
                            (!borrowed).then(|| Arc::clone(&database)),
                            transaction.clone(),
                            Arc::clone(&runtime),
                        );
                        let marker = manager.successor_batch_marker.clone();
                        if operation == ProjectedBatchOperation::Delete {
                            manager
                                .delete_many(py, PyList::empty(py).into_any())
                                .unwrap();
                        } else {
                            let rows = PyList::empty(py).into_any();
                            let output = match operation {
                                ProjectedBatchOperation::Insert => manager.insert_many(py, rows),
                                ProjectedBatchOperation::Put => manager.put_many(py, rows),
                                ProjectedBatchOperation::Update => manager.update_many(py, rows),
                                ProjectedBatchOperation::Delete => unreachable!(),
                            }
                            .unwrap();
                            assert_eq!(output.bind(py).cast::<PyList>().unwrap().len(), 0);
                        }
                        if let Some(transaction) = transaction {
                            assert_eq!(
                                runtime.block_on(transaction.lifecycle_state()),
                                TransactionContextState::Active,
                            );
                            assert!(!marker.unwrap().load(Ordering::Acquire));
                        } else {
                            assert!(marker.is_none());
                        }
                        let state = state.lock().unwrap();
                        assert!(state.responses.is_empty());
                        assert_eq!(
                            state.opens,
                            if borrowed {
                                vec![TxType::Write]
                            } else {
                                vec![]
                            },
                        );
                        assert!(state.calls.is_empty());
                        assert_eq!(state.legacy_commits, 0);
                        assert_eq!(state.commits, 0);
                        assert_eq!(state.rollbacks, 0);
                    }
                }
            }
            assert_eq!(package.facade_origins.len(), 0);
            unrelated_attribute.delattr("_attribute_value").unwrap();
        });
    }

    #[test]
    fn borrowed_successor_nonempty_preflight_failure_keeps_legacy_commit_marker_unset() {
        Python::initialize();
        Python::attach(|py| {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Delete,
            ] {
                let (_, package) = install_batch(py);
                let (database, state) = BatchRecordingBackend::fixture(vec![], false);
                let runtime =
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
                let transaction = runtime
                    .block_on(database.transaction_context(TxType::Read))
                    .unwrap();
                let context = PyRustTransactionContext::from_parts_for_test(
                    transaction.clone(),
                    Arc::clone(&runtime),
                );
                let marker = context.successor_batch_marker();
                let manager = PyProjectedModelManager {
                    package: Arc::clone(&package),
                    type_id: package
                        .type_by_label("person", TypeKind::Entity)
                        .unwrap()
                        .clone(),
                    database: None,
                    transaction: Some(transaction.clone()),
                    successor_batch_marker: Some(Arc::clone(&marker)),
                    runtime: Arc::clone(&runtime),
                    filters: Vec::new(),
                    compatibility_filter: None,
                };
                let error = if operation == ProjectedBatchOperation::Insert {
                    let input = batch_person(py, package.as_ref(), "person-0", "authored-0", None);
                    manager
                        .insert_many(py, PyList::new(py, [input.bind(py)]).unwrap().into_any())
                        .unwrap_err()
                } else {
                    manager
                        .delete_many(py, PyList::new(py, ["0x20"]).unwrap().into_any())
                        .unwrap_err()
                };
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "invalid_input",
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    "transaction_type_mismatch",
                );
                assert_eq!(
                    runtime.block_on(transaction.lifecycle_state()),
                    TransactionContextState::Active,
                );
                assert!(!marker.load(Ordering::Acquire));
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Read]);
                    assert!(state.responses.is_empty());
                    assert!(state.calls.is_empty());
                    assert_eq!(state.legacy_commits, 0);
                    assert_eq!(state.commits, 0);
                    assert_eq!(state.rollbacks, 0);
                }

                Py::new(py, context)
                    .unwrap()
                    .bind(py)
                    .call_method0("commit")
                    .unwrap();
                let state = state.lock().unwrap();
                assert_eq!(state.legacy_commits, 1);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
        });
    }

    #[test]
    fn predecessor_public_borrowed_nonempty_batches_keep_legacy_routes_and_commit_seam() {
        Python::initialize();
        Python::attach(|py| {
            for kind in [TypeKind::Entity, TypeKind::Relation] {
                for operation in [
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchOperation::Update,
                    ProjectedBatchOperation::Delete,
                ] {
                    let (_, package) = install(py);
                    let responses = match (kind, operation) {
                        (TypeKind::Entity, ProjectedBatchOperation::Insert) => {
                            vec![QueryResult::Documents(vec![serde_json::json!({
                                "iid": "0x20",
                            })])]
                        }
                        (TypeKind::Entity, ProjectedBatchOperation::Put) => vec![
                            QueryResult::Documents(vec![]),
                            QueryResult::Documents(vec![serde_json::json!({"iid": "0x20"})]),
                        ],
                        (TypeKind::Entity, ProjectedBatchOperation::Update) => vec![
                            QueryResult::Ok,
                            QueryResult::Documents(vec![legacy_person_document(
                                "0x20",
                                "legacy-person",
                            )]),
                        ],
                        (TypeKind::Entity, ProjectedBatchOperation::Delete) => {
                            vec![QueryResult::Ok]
                        }
                        (TypeKind::Relation, ProjectedBatchOperation::Insert) => {
                            vec![QueryResult::Documents(vec![serde_json::json!({
                                "iid": "0x30",
                            })])]
                        }
                        (TypeKind::Relation, ProjectedBatchOperation::Put) => vec![
                            QueryResult::Documents(vec![serde_json::json!({"iid": "0x10"})]),
                            QueryResult::Documents(vec![serde_json::json!({"iid": "0x30"})]),
                        ],
                        (TypeKind::Relation, ProjectedBatchOperation::Update) => vec![
                            QueryResult::Documents(vec![serde_json::json!({"iid": "0x10"})]),
                            QueryResult::Ok,
                            QueryResult::Ok,
                            QueryResult::Documents(vec![legacy_membership_document(
                                "0x30",
                                "0x10",
                                "legacy-player",
                            )]),
                        ],
                        (TypeKind::Relation, ProjectedBatchOperation::Delete) => {
                            vec![QueryResult::Ok]
                        }
                        _ => unreachable!(),
                    };
                    let expected_queries = responses.len();
                    let (database, state) = LegacyBatchRecordingBackend::fixture(responses);
                    let runtime = Arc::new(
                        ProviderRuntimeOwner::new().expect("provider runtime should start"),
                    );
                    let transaction = runtime
                        .block_on(database.transaction_context(TxType::Write))
                        .unwrap();
                    let context = PyRustTransactionContext::from_parts_for_test(
                        transaction.clone(),
                        Arc::clone(&runtime),
                    );
                    let marker = context.successor_batch_marker();
                    let label = if kind == TypeKind::Entity {
                        "person"
                    } else {
                        "membership"
                    };
                    let manager = PyProjectedModelManager {
                        package: Arc::clone(&package),
                        type_id: package.type_by_label(label, kind).unwrap().clone(),
                        database: None,
                        transaction: Some(transaction.clone()),
                        successor_batch_marker: Some(Arc::clone(&marker)),
                        runtime: Arc::clone(&runtime),
                        filters: Vec::new(),
                        compatibility_filter: None,
                    };
                    assert!(!manager.uses_successor_runtime());

                    let player = (kind == TypeKind::Relation)
                        .then(|| legacy_person(py, package.as_ref(), "legacy-player", None));
                    let input = if kind == TypeKind::Entity {
                        legacy_person(
                            py,
                            package.as_ref(),
                            "legacy-person",
                            (operation == ProjectedBatchOperation::Update).then_some("0x20"),
                        )
                    } else {
                        legacy_membership(
                            py,
                            package.as_ref(),
                            player.as_ref().unwrap(),
                            (operation == ProjectedBatchOperation::Update).then_some("0x30"),
                        )
                    };
                    let pointer = input.bind(py).as_ptr();
                    if operation == ProjectedBatchOperation::Delete {
                        let iid = if kind == TypeKind::Entity {
                            "0x20"
                        } else {
                            "0x30"
                        };
                        manager
                            .delete_many(py, PyList::new(py, [iid]).unwrap().into_any())
                            .unwrap();
                    } else {
                        let rows = PyList::new(py, [input.bind(py)]).unwrap().into_any();
                        let output = match operation {
                            ProjectedBatchOperation::Insert => manager.insert_many(py, rows),
                            ProjectedBatchOperation::Put => manager.put_many(py, rows),
                            ProjectedBatchOperation::Update => manager.update_many(py, rows),
                            ProjectedBatchOperation::Delete => unreachable!(),
                        }
                        .unwrap();
                        let output = output.bind(py).cast::<PyList>().unwrap();
                        assert_eq!(output.len(), 1);
                        assert_eq!(output.get_item(0).unwrap().as_ptr(), pointer);
                        assert_eq!(
                            output
                                .get_item(0)
                                .unwrap()
                                .getattr("iid")
                                .unwrap()
                                .extract::<String>()
                                .unwrap(),
                            if kind == TypeKind::Entity {
                                "0x20"
                            } else {
                                "0x30"
                            },
                        );
                    }
                    assert_eq!(
                        runtime.block_on(transaction.lifecycle_state()),
                        TransactionContextState::Active,
                    );
                    assert!(!marker.load(Ordering::Acquire));
                    {
                        let state = state.lock().unwrap();
                        assert!(state.responses.is_empty());
                        assert_eq!(state.opens, [TxType::Write]);
                        assert_eq!(state.queries.len(), expected_queries);
                        match operation {
                            ProjectedBatchOperation::Insert => {
                                assert!(state.queries.iter().any(|query| query.contains("insert")));
                            }
                            ProjectedBatchOperation::Put => {
                                assert!(state.queries.len() >= 2);
                                assert!(state.queries.last().unwrap().contains("insert"));
                            }
                            ProjectedBatchOperation::Update => {
                                assert!(state.queries.iter().any(|query| query.contains("delete")));
                                assert!(state.queries.last().unwrap().contains("fetch"));
                            }
                            ProjectedBatchOperation::Delete => {
                                assert!(state.queries[0].contains("delete"));
                            }
                        }
                        assert_eq!(state.legacy_commits, 0);
                        assert_eq!(state.sdk_commits, 0);
                        assert_eq!(state.rollbacks, 0);
                    }

                    Py::new(py, context)
                        .unwrap()
                        .bind(py)
                        .call_method0("commit")
                        .unwrap();
                    let state = state.lock().unwrap();
                    assert_eq!(state.legacy_commits, 1);
                    assert_eq!(state.sdk_commits, 0);
                    assert_eq!(state.rollbacks, 0);
                }
            }
        });
    }

    #[test]
    fn borrowed_successor_batch_marks_clone_shared_public_commit_sdk() {
        Python::initialize();
        Python::attach(|py| {
            for fail_commit in [false, true] {
                let (_, package) = install_batch(py);
                let (database, state) = BatchRecordingBackend::fixture(
                    entity_batch_responses(ProjectedBatchOperation::Insert),
                    fail_commit,
                );
                let runtime =
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
                let transaction = runtime
                    .block_on(database.transaction_context(TxType::Write))
                    .unwrap();
                let context = PyRustTransactionContext::from_parts_for_test(
                    transaction.clone(),
                    Arc::clone(&runtime),
                );
                let marker = context.successor_batch_marker();
                let manager = PyProjectedModelManager {
                    package: Arc::clone(&package),
                    type_id: package
                        .type_by_label("person", TypeKind::Entity)
                        .unwrap()
                        .clone(),
                    database: None,
                    transaction: Some(transaction),
                    successor_batch_marker: Some(Arc::clone(&marker)),
                    runtime: Arc::clone(&runtime),
                    filters: Vec::new(),
                    compatibility_filter: None,
                };
                let filtered = manager.filter(py, None).unwrap();
                let instances = (0..2)
                    .map(|ordinal| {
                        batch_person(
                            py,
                            package.as_ref(),
                            &format!("person-{ordinal}"),
                            &format!("authored-{ordinal}"),
                            None,
                        )
                    })
                    .collect::<Vec<_>>();
                let rows =
                    PyList::new(py, instances.iter().map(|instance| instance.bind(py))).unwrap();
                let output = filtered.insert_many(py, rows.into_any()).unwrap();
                assert_eq!(output.bind(py).cast::<PyList>().unwrap().len(), 2);
                assert!(marker.load(Ordering::Acquire));

                let context = Py::new(py, context).unwrap();
                let commit = context.bind(py).call_method0("commit");
                if fail_commit {
                    let error = commit.unwrap_err();
                    assert_eq!(
                        error
                            .value(py)
                            .getattr("code")
                            .unwrap()
                            .extract::<String>()
                            .unwrap(),
                        "commit_definitely_aborted",
                    );
                    assert!(!error.to_string().contains("injected commit failure"));
                } else {
                    commit.unwrap();
                }

                let state = state.lock().unwrap();
                assert!(state.responses.is_empty());
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.calls.len(), 2);
                assert_eq!(state.legacy_commits, 0);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }
        });
    }

    #[test]
    fn borrowed_successor_write_and_delete_publish_marker_before_concurrent_commit() {
        Python::initialize();
        Python::attach(|py| {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Delete,
            ] {
                let (_, package) = install_batch(py);
                let (database, state, gate) =
                    BatchRecordingBackend::gated_fixture(entity_batch_responses(operation));
                let runtime =
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
                let transaction = runtime
                    .block_on(database.transaction_context(TxType::Write))
                    .unwrap();
                let context = PyRustTransactionContext::from_parts_for_test(
                    transaction.clone(),
                    Arc::clone(&runtime),
                );
                let marker = context.successor_batch_marker();
                let manager = PyProjectedModelManager {
                    package: Arc::clone(&package),
                    type_id: package
                        .type_by_label("person", TypeKind::Entity)
                        .unwrap()
                        .clone(),
                    database: None,
                    transaction: Some(transaction),
                    successor_batch_marker: Some(Arc::clone(&marker)),
                    runtime: Arc::clone(&runtime),
                    filters: Vec::new(),
                    compatibility_filter: None,
                };
                let context = Py::new(py, context).unwrap();
                let commit_context = context.clone_ref(py);
                let commit_gate = Arc::clone(&gate);
                let (attempted_tx, attempted_rx) = std::sync::mpsc::sync_channel(0);
                let commit_thread = std::thread::spawn(move || {
                    commit_gate.wait_until_entered();
                    attempted_tx.send(()).unwrap();
                    Python::attach(|py| {
                        commit_context
                            .bind(py)
                            .call_method0("commit")
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                });
                let release_gate = Arc::clone(&gate);
                let release_state = Arc::clone(&state);
                let release_marker = Arc::clone(&marker);
                let release_thread = std::thread::spawn(move || {
                    release_gate.wait_until_entered();
                    attempted_rx.recv().unwrap();
                    assert!(!release_marker.load(Ordering::Acquire));
                    let state = release_state.lock().unwrap();
                    assert_eq!(state.legacy_commits, 0);
                    assert_eq!(state.commits, 0);
                    drop(state);
                    release_gate.release();
                });

                match operation {
                    ProjectedBatchOperation::Insert => {
                        let instances = (0..2)
                            .map(|ordinal| {
                                batch_person(
                                    py,
                                    package.as_ref(),
                                    &format!("person-{ordinal}"),
                                    &format!("authored-{ordinal}"),
                                    None,
                                )
                            })
                            .collect::<Vec<_>>();
                        let rows =
                            PyList::new(py, instances.iter().map(|instance| instance.bind(py)))
                                .unwrap();
                        let output = manager.insert_many(py, rows.into_any()).unwrap();
                        assert_eq!(output.bind(py).cast::<PyList>().unwrap().len(), 2);
                    }
                    ProjectedBatchOperation::Delete => manager
                        .delete_many(py, PyList::new(py, ["0x20", "0x21"]).unwrap().into_any())
                        .unwrap(),
                    _ => unreachable!(),
                }
                let commit = py.detach(move || {
                    release_thread.join().unwrap();
                    commit_thread.join().unwrap()
                });
                commit.unwrap();
                assert!(marker.load(Ordering::Acquire));

                let state = state.lock().unwrap();
                assert!(state.responses.is_empty());
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.legacy_commits, 0);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
                assert_eq!(
                    state.calls.len(),
                    if operation == ProjectedBatchOperation::Insert {
                        2
                    } else {
                        1
                    },
                );
            }
        });
    }

    #[test]
    fn successor_public_iterators_preserve_errors_and_enforce_the_hard_cap() {
        use pythonize::depythonize;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                Arc::clone(&package),
                package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let helpers = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
class IteratorSentinel(Exception):
    pass

class IterTypeError:
    def __iter__(self):
        raise TypeError("injected __iter__ TypeError")

class BoundaryIterator:
    def __init__(self, value, count, message):
        self.value = value
        self.count = count
        self.message = message
        self.seen = 0
    def __iter__(self):
        return self
    def __next__(self):
        if self.seen < self.count:
            self.seen += 1
            return self.value
        raise IteratorSentinel(self.message)
"#
                ),
                ffi::c_str!("python_batch_iterators.py"),
                ffi::c_str!("python_batch_iterators"),
            )
            .unwrap();

            for delete in [false, true] {
                let error = if delete {
                    manager.delete_many(py, py.None().into_bound(py))
                } else {
                    manager
                        .insert_many(py, py.None().into_bound(py))
                        .map(|_| ())
                }
                .unwrap_err();
                assert_eq!(
                    error
                        .value(py)
                        .getattr("code")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "batch_rows_not_iterable",
                );
                let path: serde_json::Value =
                    depythonize(&error.value(py).getattr("path").unwrap()).unwrap();
                assert_eq!(
                    path,
                    serde_json::json!([{"kind": "argument", "value": "rows"}]),
                );

                let hostile = helpers.getattr("IterTypeError").unwrap().call0().unwrap();
                let error = if delete {
                    manager.delete_many(py, hostile)
                } else {
                    manager.insert_many(py, hostile).map(|_| ())
                }
                .unwrap_err();
                assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
                assert!(error.to_string().contains("injected __iter__ TypeError"));
                assert!(error.value(py).getattr("code").is_err());
            }

            let maximum = usize::try_from(type_bridge_orm::MAX_QUERY_ITEMS).unwrap();
            let facade = batch_person(py, package.as_ref(), "boundary", "boundary", None);
            for (delete, item, message) in [
                (false, facade.clone_ref(py), "facade boundary"),
                (
                    true,
                    "0x20".into_pyobject(py).unwrap().into_any().unbind(),
                    "iid boundary",
                ),
            ] {
                let boundary = helpers
                    .getattr("BoundaryIterator")
                    .unwrap()
                    .call1((item.bind(py), maximum, message))
                    .unwrap();
                let error = if delete {
                    manager.delete_many(py, boundary)
                } else {
                    manager.insert_many(py, boundary).map(|_| ())
                }
                .unwrap_err();
                assert_eq!(error.get_type(py).name().unwrap(), "IteratorSentinel");
                assert!(error.to_string().contains(message));
                assert!(error.value(py).getattr("code").is_err());

                let over_limit = helpers
                    .getattr("BoundaryIterator")
                    .unwrap()
                    .call1((item.bind(py), maximum + 1, "unreachable sentinel"))
                    .unwrap();
                let error = if delete {
                    manager.delete_many(py, over_limit)
                } else {
                    manager.insert_many(py, over_limit).map(|_| ())
                }
                .unwrap_err();
                assert_eq!(
                    error
                        .value(py)
                        .getattr("code")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "batch_item_limit",
                );
                let path: serde_json::Value =
                    depythonize(&error.value(py).getattr("path").unwrap()).unwrap();
                assert_eq!(
                    path,
                    serde_json::json!([
                        {"kind": "argument", "value": "limits"},
                        {"kind": "argument", "value": "items"},
                    ]),
                );
            }

            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.calls.is_empty());
            assert_eq!(state.legacy_commits, 0);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn successor_batch_preflight_keeps_common_row_diagnostics_and_precedes_io() {
        use pythonize::depythonize;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let duplicate = batch_person(py, package.as_ref(), "duplicate", "first", None);
            let error = manager
                .insert_many(
                    py,
                    PyList::new(py, [duplicate.bind(py), duplicate.bind(py)])
                        .unwrap()
                        .into_any(),
                )
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "invalid_input",
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "duplicate_batch_key",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path[0],
                serde_json::json!({"kind": "argument", "value": "rows"})
            );
            assert_eq!(path[1], serde_json::json!({"kind": "index", "value": 1}));
            assert_eq!(path[2]["kind"], "field");
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({
                    "first_conflicting_index": {"kind": "count", "value": 0},
                }),
            );
            {
                let state = state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.calls.is_empty());
            }
            assert_eq!(package.facade_origins.len(), 0);

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let duplicate_target =
                batch_person(py, package.as_ref(), "first", "first", Some("0x20"));
            let error = manager
                .update_many(
                    py,
                    PyList::new(py, [duplicate_target.bind(py), duplicate_target.bind(py)])
                        .unwrap()
                        .into_any(),
                )
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "duplicate_batch_target",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([
                    {"kind": "argument", "value": "rows"},
                    {"kind": "index", "value": 1},
                    {"kind": "argument", "value": "iid"},
                ]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({
                    "first_conflicting_index": {"kind": "count", "value": 0},
                }),
            );
            {
                let state = state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.calls.is_empty());
            }

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let memo_id = package
                .type_by_label("memo", TypeKind::Entity)
                .unwrap()
                .clone();
            let manager = origin_manager(
                Arc::clone(&package),
                memo_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let memo = package
                .class(&memo_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call0()
                .unwrap();
            let error = manager
                .insert_many(
                    py,
                    PyList::new(py, [memo.clone(), memo]).unwrap().into_any(),
                )
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "duplicate_batch_input_identity",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([
                    {"kind": "argument", "value": "rows"},
                    {"kind": "index", "value": 1},
                ]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({
                    "first_conflicting_index": {"kind": "count", "value": 0},
                }),
            );
            {
                let state = state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.calls.is_empty());
            }

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                Arc::clone(&package),
                package
                    .type_by_label("membership", TypeKind::Relation)
                    .unwrap()
                    .clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let error = manager
                .delete_many_projected(py, vec!["0x30".into(), "0x30".into()])
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "duplicate_batch_target",
            );
            {
                let state = state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.calls.is_empty());
            }

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let valid = batch_person(py, package.as_ref(), "valid", "valid", None);
            let invalid = batch_person(py, package.as_ref(), "invalid", "duplicate", None);
            let values = invalid
                .bind(py)
                .getattr("_values")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            let tags = values
                .get_item("tag")
                .unwrap()
                .unwrap()
                .cast_into::<PyTuple>()
                .unwrap();
            let tag = tags.get_item(0).unwrap();
            values
                .set_item("tag", PyTuple::new(py, [tag.clone(), tag]).unwrap())
                .unwrap();
            let error = manager
                .write_many_projected(py, vec![valid, invalid], ProjectedBatchOperation::Insert)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "ordered_distinct_duplicate",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path[0],
                serde_json::json!({"kind": "argument", "value": "rows"})
            );
            assert_eq!(path[1], serde_json::json!({"kind": "index", "value": 1}));
            {
                let state = state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.calls.is_empty());
            }
            assert_eq!(package.facade_origins.len(), 0);

            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let manager = origin_manager(
                package,
                person_id,
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let row_count = usize::try_from(type_bridge_orm::MAX_QUERY_ITEMS).unwrap() + 1;
            let hostile = (0..row_count).map(|_| py.None()).collect::<Vec<_>>();
            let error = manager
                .write_many_projected(py, hostile, ProjectedBatchOperation::Insert)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "resource_limit",
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "batch_item_limit",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([
                    {"kind": "argument", "value": "limits"},
                    {"kind": "argument", "value": "items"},
                ]),
            );
            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.calls.is_empty());
        });
    }

    #[test]
    fn owned_successor_provider_and_commit_failures_are_exact_and_terminal() {
        use pythonize::depythonize;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let (database, state) =
                BatchRecordingBackend::fixture(vec![BatchResponse::Error], false);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let input = batch_person(py, package.as_ref(), "person-0", "authored-0", None);
            let retained = input.clone_ref(py);
            let prior_values = retained.bind(py).getattr("_values").unwrap().unbind();
            let prior_iid = retained.bind(py).getattr("_iid").unwrap().unbind();
            let error = manager
                .write_many_projected(py, vec![input], ProjectedBatchOperation::Insert)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "provider",
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "provider_operation_failed",
            );
            assert!(!error.to_string().contains("injected batch failure"));
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([{"kind": "type", "value": person_id}]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({"operation": {"kind": "text", "value": "write"}}),
            );
            assert!(
                retained
                    .bind(py)
                    .getattr("_values")
                    .unwrap()
                    .is(prior_values.bind(py))
            );
            assert!(
                retained
                    .bind(py)
                    .getattr("_iid")
                    .unwrap()
                    .is(prior_iid.bind(py))
            );
            assert_eq!(package.facade_origins.len(), 0);
            {
                let state = state.lock().unwrap();
                assert!(state.responses.is_empty());
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.calls.len(), 1);
                assert_eq!(state.legacy_commits, 0);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 1);
            }

            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let (database, state) = BatchRecordingBackend::fixture(
                entity_batch_responses(ProjectedBatchOperation::Insert),
                true,
            );
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(database),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let inputs = (0..2)
                .map(|ordinal| {
                    batch_person(
                        py,
                        package.as_ref(),
                        &format!("person-{ordinal}"),
                        &format!("authored-{ordinal}"),
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let retained = inputs
                .iter()
                .map(|input| input.clone_ref(py))
                .collect::<Vec<_>>();
            let prior_slots = retained
                .iter()
                .map(|input| {
                    (
                        input.bind(py).getattr("_values").unwrap().unbind(),
                        input.bind(py).getattr("_iid").unwrap().unbind(),
                    )
                })
                .collect::<Vec<_>>();
            let error = manager
                .write_many_projected(py, inputs, ProjectedBatchOperation::Insert)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "transaction",
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "commit_definitely_aborted",
            );
            assert!(!error.to_string().contains("injected commit failure"));
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([{"kind": "type", "value": person_id}]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({
                    "commit_outcome": {"kind": "text", "value": "definitely_aborted"},
                }),
            );
            for (input, (prior_values, prior_iid)) in retained.iter().zip(&prior_slots) {
                assert!(
                    input
                        .bind(py)
                        .getattr("_values")
                        .unwrap()
                        .is(prior_values.bind(py)),
                );
                assert!(
                    input
                        .bind(py)
                        .getattr("_iid")
                        .unwrap()
                        .is(prior_iid.bind(py)),
                );
                assert!(package.facade_origins.proof(input.bind(py)).is_none());
            }
            assert_eq!(package.facade_origins.len(), 0);
            let state = state.lock().unwrap();
            assert!(state.responses.is_empty());
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.calls.len(), 2);
            assert_eq!(state.legacy_commits, 0);
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn unmarked_python_transaction_commit_keeps_the_legacy_error_surface() {
        Python::initialize();
        Python::attach(|py| {
            let (database, state) = BatchRecordingBackend::fixture(vec![], false);
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let transaction = runtime
                .block_on(database.transaction_context(TxType::Write))
                .unwrap();
            let context =
                PyRustTransactionContext::from_parts_for_test(transaction, Arc::clone(&runtime));
            assert!(!context.successor_batch_marker().load(Ordering::Acquire));
            Py::new(py, context)
                .unwrap()
                .bind(py)
                .call_method0("commit")
                .unwrap();
            {
                let state = state.lock().unwrap();
                assert_eq!(state.legacy_commits, 1);
                assert_eq!(state.commits, 0);
            }

            let transaction = runtime
                .block_on(database.transaction_context(TxType::Write))
                .unwrap();
            runtime.block_on(
                transaction.latch_rollback_only(&SdkExecutionDiagnostic::internal_failure()),
            );
            let context =
                PyRustTransactionContext::from_parts_for_test(transaction, Arc::clone(&runtime));
            let error = Py::new(py, context)
                .unwrap()
                .bind(py)
                .call_method0("commit")
                .unwrap_err();
            assert!(error.value(py).getattr("sdk_category").is_err());
            assert!(error.value(py).getattr("code").is_err());
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write, TxType::Write]);
            assert_eq!(state.legacy_commits, 1);
            assert_eq!(state.commits, 0);
        });
    }

    #[test]
    fn projected_batch_mapper_publishes_original_identities_only_after_total_success() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let originals = (0..3)
                .map(|ordinal| {
                    batch_person(
                        py,
                        package.as_ref(),
                        &format!("authored-{ordinal}"),
                        &format!("authored-tag-{ordinal}"),
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let pointers = originals
                .iter()
                .map(|value| value.bind(py).as_ptr())
                .collect::<Vec<_>>();
            let facades = prepared_batch_facades(py, package.as_ref(), originals);
            let projected = projected_batch_people(py, package.as_ref(), 3);
            let binding_error = Arc::new(Mutex::new(None));
            let slots = package
                .batch_slots(package.type_by_label("person", TypeKind::Entity).unwrap())
                .unwrap();
            let pool = hydration_pool_for_prepared(py, &facades);
            let output = materialize_projected_batch(
                py,
                Arc::clone(&package),
                slots,
                facades,
                ProjectedBatchResult::Things(projected),
                &binding_error,
                &pool,
            )
            .unwrap_or_else(|diagnostic| panic!("materialization failed: {diagnostic:?}"))
            .finish();
            assert!(take_binding_materialization_error(&binding_error).is_none());
            let output = output.bind(py).cast::<PyList>().unwrap();
            assert_eq!(output.len(), 3);
            for (ordinal, pointer) in pointers.into_iter().enumerate() {
                let value = output.get_item(ordinal).unwrap();
                assert_eq!(value.as_ptr(), pointer);
                assert_eq!(
                    value.getattr("iid").unwrap().extract::<String>().unwrap(),
                    format!("0x2{ordinal}"),
                );
                let values = value
                    .call_method0("runtime_values")
                    .unwrap()
                    .cast_into::<PyDict>()
                    .unwrap();
                let tag = values
                    .get_item("tag")
                    .unwrap()
                    .unwrap()
                    .cast_into::<PyTuple>()
                    .unwrap()
                    .get_item(0)
                    .unwrap();
                assert_eq!(
                    tag.call_method0("runtime_attribute_value")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    format!("normalized-{ordinal}"),
                );
                assert!(matches!(
                    package.facade_origins.proof(&value),
                    Some(FacadeProjectionProof::Thing(_)),
                ));
            }
            assert_eq!(package.facade_origins.len(), 3);
        });
    }

    #[test]
    fn projected_batch_mapper_restores_every_prefix_and_publishes_no_proof_on_failure() {
        Python::initialize();
        Python::attach(|py| {
            for failing_ordinal in 0..3 {
                let (_, package) = install_batch(py);
                let originals = (0..3)
                    .map(|ordinal| {
                        batch_person(
                            py,
                            package.as_ref(),
                            &format!("authored-{ordinal}"),
                            &format!("authored-tag-{ordinal}"),
                            None,
                        )
                    })
                    .collect::<Vec<_>>();
                let prior = originals
                    .iter()
                    .map(|value| {
                        (
                            value.bind(py).getattr("_values").unwrap().unbind(),
                            value.bind(py).getattr("_iid").unwrap().unbind(),
                        )
                    })
                    .collect::<Vec<_>>();
                let facades = prepared_batch_facades(
                    py,
                    package.as_ref(),
                    originals.iter().map(|value| value.clone_ref(py)).collect(),
                );
                let projected = projected_batch_people(py, package.as_ref(), 3);
                let binding_error = Arc::new(Mutex::new(None));
                let slots = package
                    .batch_slots(package.type_by_label("person", TypeKind::Entity).unwrap())
                    .unwrap();
                let pool = hydration_pool_for_prepared(py, &facades);
                let diagnostic = match materialize_projected_batch_with_probe(
                    py,
                    Arc::clone(&package),
                    slots,
                    facades,
                    ProjectedBatchResult::Things(projected),
                    &binding_error,
                    &pool,
                    BatchMaterializationProbe {
                        fail_publication_at: Some(failing_ordinal),
                        ..BatchMaterializationProbe::default()
                    },
                ) {
                    Err(diagnostic) => diagnostic,
                    Ok(_) => panic!("injected publication failure unexpectedly succeeded"),
                };
                assert_eq!(
                    diagnostic.code().as_str(),
                    "generated_model_materialization_failed"
                );
                assert!(matches!(
                    diagnostic.path(),
                    [SdkDiagnosticPathSegment::Argument(argument), SdkDiagnosticPathSegment::Index(index)]
                        if argument.as_str() == "rows" && *index == failing_ordinal as u64
                ));
                let error = take_binding_materialization_error(&binding_error).unwrap();
                assert!(
                    error
                        .to_string()
                        .contains("injected projected batch publication failure")
                );
                assert_eq!(package.facade_origins.len(), 0);
                for (facade, (values, iid)) in originals.iter().zip(prior.iter()) {
                    assert!(
                        facade
                            .bind(py)
                            .getattr("_values")
                            .unwrap()
                            .is(values.bind(py))
                    );
                    assert!(facade.bind(py).getattr("_iid").unwrap().is(iid.bind(py)));
                }
            }

            let (_, package) = install_batch(py);
            let originals = (0..3)
                .map(|ordinal| {
                    batch_person(
                        py,
                        package.as_ref(),
                        &format!("allocation-{ordinal}"),
                        &format!("allocation-tag-{ordinal}"),
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let prior = originals
                .iter()
                .map(|value| {
                    (
                        value.bind(py).getattr("_values").unwrap().unbind(),
                        value.bind(py).getattr("_iid").unwrap().unbind(),
                    )
                })
                .collect::<Vec<_>>();
            let facades = prepared_batch_facades(
                py,
                package.as_ref(),
                originals.iter().map(|value| value.clone_ref(py)).collect(),
            );
            let projected = projected_batch_people(py, package.as_ref(), 3);
            let binding_error = Arc::new(Mutex::new(None));
            let slots = package
                .batch_slots(package.type_by_label("person", TypeKind::Entity).unwrap())
                .unwrap();
            let pool = hydration_pool_for_prepared(py, &facades);
            let diagnostic = match materialize_projected_batch_with_probe(
                py,
                Arc::clone(&package),
                slots,
                facades,
                ProjectedBatchResult::Things(projected),
                &binding_error,
                &pool,
                BatchMaterializationProbe {
                    fail_output_allocation: true,
                    ..BatchMaterializationProbe::default()
                },
            ) {
                Err(diagnostic) => diagnostic,
                Ok(_) => panic!("injected output allocation failure unexpectedly succeeded"),
            };
            assert_eq!(
                diagnostic.code().as_str(),
                "projected_batch_allocation_exhausted"
            );
            assert!(take_binding_materialization_error(&binding_error).is_none());
            assert_eq!(package.facade_origins.len(), 0);
            for (facade, (values, iid)) in originals.iter().zip(prior.iter()) {
                assert!(
                    facade
                        .bind(py)
                        .getattr("_values")
                        .unwrap()
                        .is(values.bind(py))
                );
                assert!(facade.bind(py).getattr("_iid").unwrap().is(iid.bind(py)));
            }
        });
    }

    #[test]
    fn python_materialization_first_middle_last_preserves_error_and_transaction_atomicity() {
        Python::initialize();
        Python::attach(|py| {
            for borrowed in [false, true] {
                for failing_ordinal in 0..3 {
                    let (_, package) = install_batch(py);
                    let (database, state) =
                        BatchRecordingBackend::fixture(entity_insert_responses(3), false);
                    let runtime = Arc::new(
                        ProviderRuntimeOwner::new().expect("provider runtime should start"),
                    );
                    let transaction = borrowed.then(|| {
                        runtime
                            .block_on(database.transaction_context(TxType::Write))
                            .unwrap()
                    });
                    let context = transaction.as_ref().map(|transaction| {
                        PyRustTransactionContext::from_parts_for_test(
                            transaction.clone(),
                            Arc::clone(&runtime),
                        )
                    });
                    let instances = (0..3)
                        .map(|ordinal| {
                            batch_person(
                                py,
                                package.as_ref(),
                                &format!("person-{ordinal}"),
                                &format!("authored-{ordinal}"),
                                None,
                            )
                        })
                        .collect::<Vec<_>>();
                    let retained = instances
                        .iter()
                        .map(|instance| instance.clone_ref(py))
                        .collect::<Vec<_>>();
                    let prior = retained
                        .iter()
                        .map(|instance| {
                            (
                                instance.bind(py).getattr("_values").unwrap().unbind(),
                                instance.bind(py).getattr("_iid").unwrap().unbind(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let marker = context
                        .as_ref()
                        .map(PyRustTransactionContext::successor_batch_marker);
                    let error = execute_batch_with_materialization_probe(
                        py,
                        Arc::clone(&package),
                        package
                            .type_by_label("person", TypeKind::Entity)
                            .unwrap()
                            .clone(),
                        (!borrowed).then(|| Arc::clone(&database)),
                        transaction.clone(),
                        marker.clone(),
                        Arc::clone(&runtime),
                        instances,
                        BatchMaterializationProbe {
                            fail_publication_at: Some(failing_ordinal),
                            ..BatchMaterializationProbe::default()
                        },
                    )
                    .unwrap_err();
                    assert!(
                        error
                            .to_string()
                            .contains("injected projected batch publication failure"),
                        "unexpected caller error: {error}",
                    );
                    assert!(error.value(py).getattr("sdk_category").is_err());
                    for (instance, (values, iid)) in retained.iter().zip(prior.iter()) {
                        assert!(
                            instance
                                .bind(py)
                                .getattr("_values")
                                .unwrap()
                                .is(values.bind(py))
                        );
                        assert!(instance.bind(py).getattr("_iid").unwrap().is(iid.bind(py)));
                    }
                    assert_eq!(package.facade_origins.len(), 0);

                    if let Some(transaction) = transaction {
                        assert!(marker.unwrap().load(Ordering::Acquire));
                        assert_eq!(
                            runtime.block_on(transaction.lifecycle_state()),
                            TransactionContextState::RollbackOnly,
                        );
                        let cause = runtime
                            .block_on(transaction.rollback_only_cause())
                            .expect("borrowed materialization failure retains its common cause");
                        assert_eq!(
                            cause.code().as_str(),
                            "generated_model_materialization_failed"
                        );
                        assert!(matches!(
                            cause.path(),
                            [SdkDiagnosticPathSegment::Argument(argument), SdkDiagnosticPathSegment::Index(index)]
                                if argument.as_str() == "rows" && *index == failing_ordinal as u64
                        ));

                        let context = Py::new(py, context.unwrap()).unwrap();
                        let commit = context.bind(py).call_method0("commit").unwrap_err();
                        assert_eq!(
                            commit
                                .value(py)
                                .getattr("code")
                                .unwrap()
                                .extract::<String>()
                                .unwrap(),
                            "transaction_rollback_only",
                        );
                    }
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.calls.len(), 2);
                    assert_eq!(state.legacy_commits, 0);
                    assert_eq!(state.commits, 0);
                    assert_eq!(state.rollbacks, usize::from(!borrowed));
                }
            }
        });
    }

    #[test]
    fn successor_batch_hydration_pool_forecasts_and_retains_every_allocation() {
        const NAMED_ZONE_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  observed: { value: datetime-tz }
entities:
  event:
    owns:
      observed:
        card: { min: 0, max: 2 }
        ordered: true
"#;

        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install_batch(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let membership_id = package
                .type_by_label("membership", TypeKind::Relation)
                .unwrap()
                .clone();

            let people = projected_batch_people(py, package.as_ref(), 1);
            assert_eq!(projected_batch_hydration_pool_capacity(&people).unwrap(), 3);

            let player = batch_person(
                py,
                package.as_ref(),
                "player-forecast",
                "player-tag",
                Some("0x10"),
            );
            let membership = batch_membership(
                py,
                package.as_ref(),
                "membership-forecast",
                &player,
                Some("0x30"),
            );
            let membership_values = membership
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            let membership = project_hydrated_thing(
                py,
                package.as_ref(),
                &membership_id,
                &membership_values,
                Some("0x30"),
            )
            .unwrap();
            assert_eq!(
                projected_batch_hydration_pool_capacity(std::slice::from_ref(&membership)).unwrap(),
                5,
            );

            let named_zone_authority =
                authority(NAMED_ZONE_SCHEMA, "python-batch-named-zone-forecast.yaml");
            let named_zone_projection = python_projection(&named_zone_authority);
            let named_zone_installed =
                InstalledRuntimeProjection::try_new(named_zone_projection).unwrap();
            let observed_id = TypeId::new(TypeKind::Attribute, "observed").unwrap();
            let local: CanonicalDateTime = "2026-08-14T12:30:00".parse().unwrap();
            let named_zone = ProjectedAttributeValue::try_new(
                &named_zone_installed,
                observed_id.clone(),
                CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_named_resolved(local, "Europe/London", 3_600).unwrap(),
                ),
            )
            .unwrap();
            let fixed_zone = ProjectedAttributeValue::try_new(
                &named_zone_installed,
                observed_id,
                CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_fixed(local, TimeZoneDesignator::Utc).unwrap(),
                ),
            )
            .unwrap();
            let mut named_zone_count = 0;
            add_projected_attribute_pool_objects(
                &mut named_zone_count,
                [&named_zone, &fixed_zone].into_iter(),
            )
            .unwrap();
            assert_eq!(named_zone_count, 3);

            let original = batch_person(
                py,
                package.as_ref(),
                "pool-original",
                "pool-original-tag",
                None,
            );
            let slots = package.batch_slots(&person_id).unwrap();
            let staged = vec![StagedBatchFacade {
                snapshot: slots.snapshot(py, original.bind(py)).unwrap(),
                instance: original.clone_ref(py),
            }];
            let expected = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);

            let exact = BatchHydrationPool::try_new(&staged).unwrap();
            exact.reserve_exact(1).unwrap();
            let fresh = batch_person(py, package.as_ref(), "pool-fresh", "pool-fresh-tag", None);
            exact.retain_fresh(fresh.bind(py), expected).unwrap();
            exact.verify_fully_consumed().unwrap();

            let aliased = BatchHydrationPool::try_new(&staged).unwrap();
            aliased.reserve_exact(1).unwrap();
            let error = aliased
                .retain_fresh(original.bind(py), expected)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("input or repeated object identity")
            );

            let overflow = BatchHydrationPool::try_new(&[]).unwrap();
            overflow.reserve_exact(0).unwrap();
            let overflow_value = batch_person(
                py,
                package.as_ref(),
                "pool-overflow",
                "pool-overflow-tag",
                None,
            );
            let observer = PyWeakrefReference::new(overflow_value.bind(py))
                .unwrap()
                .unbind();
            let error = overflow
                .retain_fresh(overflow_value.bind(py), expected)
                .unwrap_err();
            assert!(error.to_string().contains("prevalidated object forecast"));
            drop(overflow_value);
            assert!(observer.bind(py).upgrade().is_some());
            drop(overflow);
            assert!(observer.bind(py).upgrade().is_none());

            let exhausted = BatchHydrationPool::try_new(&[])
                .unwrap()
                .reserve_exact(usize::MAX)
                .unwrap_err();
            assert_eq!(
                exhausted.code().as_str(),
                "projected_batch_allocation_exhausted"
            );
        });
    }

    #[test]
    fn successor_batch_gc_guard_restores_state_and_defers_cyclic_finalizers() {
        Python::initialize();
        Python::attach(|py| {
            let initially_enabled = unsafe { pyo3::ffi::PyGC_IsEnabled() } != 0;
            unsafe {
                pyo3::ffi::PyGC_Enable();
            }
            {
                let _guard = SuccessorBatchGcGuard::disable(py);
                assert_eq!(unsafe { pyo3::ffi::PyGC_IsEnabled() }, 0);
            }
            assert_ne!(unsafe { pyo3::ffi::PyGC_IsEnabled() }, 0);

            unsafe {
                pyo3::ffi::PyGC_Disable();
            }
            {
                let _guard = SuccessorBatchGcGuard::disable(py);
                unsafe {
                    pyo3::ffi::PyGC_Enable();
                }
            }
            assert_eq!(unsafe { pyo3::ffi::PyGC_IsEnabled() }, 0);
            unsafe {
                pyo3::ffi::PyGC_Enable();
            }

            let helper = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
class CyclicFinalizer:
    def __init__(self, seen):
        self.seen = seen
        self.cycle = self
    def __del__(self):
        self.seen.append("collected")
"#
                ),
                ffi::c_str!("python_batch_gc.py"),
                ffi::c_str!("python_batch_gc"),
            )
            .unwrap();
            let seen = PyList::empty(py);
            let cycle = helper
                .getattr("CyclicFinalizer")
                .unwrap()
                .call1((&seen,))
                .unwrap();
            {
                let _guard = SuccessorBatchGcGuard::disable(py);
                drop(cycle);
                for _ in 0..128 {
                    drop(PyDict::new(py));
                }
                assert_eq!(seen.len(), 0);
            }
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(
                seen.get_item(0).unwrap().extract::<String>().unwrap(),
                "collected"
            );

            if !initially_enabled {
                unsafe {
                    pyo3::ffi::PyGC_Disable();
                }
            }
        });
    }

    #[test]
    fn python_mapper_panic_restores_facades_gc_and_marks_borrowed_public_commit() {
        Python::initialize();
        Python::attach(|py| {
            let initially_enabled = unsafe { pyo3::ffi::PyGC_IsEnabled() } != 0;
            unsafe {
                pyo3::ffi::PyGC_Enable();
            }
            for borrowed in [false, true] {
                let (_, package) = install_batch(py);
                let (database, state) =
                    BatchRecordingBackend::fixture(entity_insert_responses(3), false);
                let runtime =
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
                let transaction = borrowed.then(|| {
                    runtime
                        .block_on(database.transaction_context(TxType::Write))
                        .unwrap()
                });
                let context = transaction.as_ref().map(|transaction| {
                    PyRustTransactionContext::from_parts_for_test(
                        transaction.clone(),
                        Arc::clone(&runtime),
                    )
                });
                let marker = context
                    .as_ref()
                    .map(PyRustTransactionContext::successor_batch_marker);
                let instances = (0..3)
                    .map(|ordinal| {
                        batch_person(
                            py,
                            package.as_ref(),
                            &format!("panic-{ordinal}"),
                            &format!("authored-{ordinal}"),
                            None,
                        )
                    })
                    .collect::<Vec<_>>();
                let retained = instances
                    .iter()
                    .map(|instance| instance.clone_ref(py))
                    .collect::<Vec<_>>();
                let prior = retained
                    .iter()
                    .map(|instance| {
                        (
                            instance.bind(py).getattr("_values").unwrap().unbind(),
                            instance.bind(py).getattr("_iid").unwrap().unbind(),
                        )
                    })
                    .collect::<Vec<_>>();
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_batch_with_materialization_probe(
                        py,
                        Arc::clone(&package),
                        package
                            .type_by_label("person", TypeKind::Entity)
                            .unwrap()
                            .clone(),
                        (!borrowed).then(|| Arc::clone(&database)),
                        transaction.clone(),
                        marker.clone(),
                        Arc::clone(&runtime),
                        instances,
                        BatchMaterializationProbe {
                            panic_publication_at: Some(1),
                            ..BatchMaterializationProbe::default()
                        },
                    )
                }))
                .expect_err("the mapper panic must resume after common cleanup");
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&'static str>().copied())
                    .unwrap_or("unknown panic payload");
                assert!(message.contains("mapper panic at row 1"));
                assert_ne!(unsafe { pyo3::ffi::PyGC_IsEnabled() }, 0);
                for (instance, (values, iid)) in retained.iter().zip(prior.iter()) {
                    assert!(
                        instance
                            .bind(py)
                            .getattr("_values")
                            .unwrap()
                            .is(values.bind(py))
                    );
                    assert!(instance.bind(py).getattr("_iid").unwrap().is(iid.bind(py)));
                }
                assert_eq!(package.facade_origins.len(), 0);

                if let Some(transaction) = transaction {
                    assert!(marker.unwrap().load(Ordering::Acquire));
                    assert_eq!(
                        runtime.block_on(transaction.lifecycle_state()),
                        TransactionContextState::RollbackOnly,
                    );
                    let cause = runtime
                        .block_on(transaction.rollback_only_cause())
                        .expect("mapper panic retains its redacted common cause");
                    assert_eq!(
                        cause.code().as_str(),
                        "generated_model_materialization_failed"
                    );
                    assert!(matches!(
                        cause.path(),
                        [SdkDiagnosticPathSegment::Argument(argument)]
                            if argument.as_str() == "rows"
                    ));
                    let context = Py::new(py, context.unwrap()).unwrap();
                    let commit = context.bind(py).call_method0("commit").unwrap_err();
                    assert_eq!(
                        commit
                            .value(py)
                            .getattr("code")
                            .unwrap()
                            .extract::<String>()
                            .unwrap(),
                        "transaction_rollback_only",
                    );
                }
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.calls.len(), 2);
                assert_eq!(state.legacy_commits, 0);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, usize::from(!borrowed));
            }
            if !initially_enabled {
                unsafe {
                    pyo3::ffi::PyGC_Disable();
                }
            }
        });
    }

    #[test]
    fn ordered_install_normalizes_all_projection_admission_failures() {
        use pythonize::depythonize;

        let ordered_authority = authority(ORDERED_SCHEMA, "python-ordered.yaml");
        let authority_bytes = encode_schema_authority(&ordered_authority);
        let exact = python_projection(&ordered_authority);
        let emitter = PythonEmitter::new();
        assert_eq!(exact.generator_handlers(), [ProjectionHandler::python_v2()]);

        let rejected = |py: Python<'_>,
                        projection_json: &str,
                        semantic_fingerprint_json: &str,
                        projection_fingerprint_json: &str,
                        authority_bytes: Option<&[u8]>| {
            install_projection(
                py,
                projection_json,
                semantic_fingerprint_json,
                projection_fingerprint_json,
                vec![],
                authority_bytes,
            )
            .err()
            .expect("hostile ordered projection evidence must fail")
        };

        let mut missing_resources = exact.code_resources().to_vec();
        missing_resources.pop();
        let missing = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &missing_resources,
        )
        .unwrap();

        let mut extra_resources = exact.code_resources().to_vec();
        extra_resources.push(
            CodeResourceDigest::from_bytes(
                "typebridge.generator.python.unexpected-resource",
                b"unexpected Python resource",
            )
            .unwrap(),
        );
        extra_resources.sort_by(|left, right| left.id().cmp(right.id()));
        let extra = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &extra_resources,
        )
        .unwrap();

        let stale = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();

        let mut forged_resources = exact.code_resources().to_vec();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged Python resource").unwrap();
        let forged = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &forged_resources,
        )
        .unwrap();

        let exact_json = String::from_utf8(to_canonical_json(&exact).unwrap()).unwrap();
        let exact_semantic =
            String::from_utf8(to_canonical_json(exact.semantic_fingerprint()).unwrap()).unwrap();
        let exact_fingerprint =
            String::from_utf8(to_canonical_json(exact.projection_fingerprint()).unwrap()).unwrap();

        let projection_parts = |projection: &RuntimeProjection| {
            (
                String::from_utf8(to_canonical_json(projection).unwrap()).unwrap(),
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap(),
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap(),
            )
        };
        let missing = projection_parts(&missing);
        let extra = projection_parts(&extra);
        let stale = projection_parts(&stale);
        let forged = projection_parts(&forged);

        let mut duplicate: serde_json::Value = serde_json::from_str(&exact_json).unwrap();
        let resources = duplicate["code_resources"].as_array_mut().unwrap();
        let repeated_resource = resources[0].clone();
        resources.insert(1, repeated_resource);
        let duplicate = String::from_utf8(to_canonical_json(&duplicate).unwrap()).unwrap();

        let mut reordered: serde_json::Value = serde_json::from_str(&exact_json).unwrap();
        reordered["code_resources"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let reordered = String::from_utf8(to_canonical_json(&reordered).unwrap()).unwrap();

        let noncanonical = serde_json::to_string_pretty(
            &serde_json::from_str::<serde_json::Value>(&exact_json).unwrap(),
        )
        .unwrap();

        let typescript = TypeScriptEmitter::new();
        let foreign_target = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &typescript.generator_handlers_for(ordered_authority.resolved_schema()),
            &typescript
                .code_resources_for(ordered_authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let foreign_target = projection_parts(&foreign_target);

        let foreign = authority(
            "format: typebridge.schema/v2\nentities:\n  foreign: {}\n",
            "python-foreign.yaml",
        );
        let foreign = encode_schema_authority(&foreign);

        Python::initialize();
        Python::attach(|py| {
            install_projection(
                py,
                &exact_json,
                &exact_semantic,
                &exact_fingerprint,
                classes(py, &exact),
                Some(&authority_bytes),
            )
            .expect("the exact ordered Python package evidence must install");

            // An empty detached first slot is the package-install API's only
            // representable absence. Translate it before decoding so all
            // managed bindings reach the common C29 classifier.
            let absent_semantic = rejected(
                py,
                &exact_json,
                "",
                &exact_fingerprint,
                Some(&authority_bytes),
            );
            let value = absent_semantic.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity",
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "projection_evidence_mismatch",
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            assert_eq!(
                path,
                serde_json::json!([
                    {"kind": "argument", "value": "projection_evidence"},
                    {"kind": "index", "value": 0},
                    {
                        "kind": "contract_identity",
                        "value": "semantic_schema_fingerprint",
                    },
                ]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                serde_json::json!({
                    "actual_occurrence_count": {"kind": "count", "value": 0},
                    "expected_occurrence_count": {"kind": "count", "value": 1},
                    "foreign_package": {"kind": "boolean", "value": false},
                }),
            );

            for error in [
                rejected(py, &exact_json, &exact_semantic, &exact_fingerprint, None),
                rejected(
                    py,
                    &missing.0,
                    &missing.1,
                    &missing.2,
                    Some(&authority_bytes),
                ),
                rejected(py, &extra.0, &extra.1, &extra.2, Some(&authority_bytes)),
                rejected(
                    py,
                    &duplicate,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &reordered,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(py, &forged.0, &forged.1, &forged.2, Some(&authority_bytes)),
                rejected(py, &stale.0, &stale.1, &stale.2, Some(&authority_bytes)),
                rejected(
                    py,
                    &noncanonical,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    "{",
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    "{",
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    "{",
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &foreign_target.0,
                    &foreign_target.1,
                    &foreign_target.2,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(b"{"),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&foreign),
                ),
            ] {
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity",
                );
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity",
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    "projection_evidence_mismatch",
                );
                assert_eq!(
                    value
                        .getattr("message")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "Generated projection evidence does not match the verified schema package",
                );
                let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
                assert_eq!(
                    path,
                    serde_json::json!([
                        {"kind": "argument", "value": "projection_evidence"}
                    ]),
                );
                let details: serde_json::Value =
                    depythonize(&value.getattr("details").unwrap()).unwrap();
                assert_eq!(details, serde_json::json!({}));
            }
        });
    }

    #[test]
    fn authorityless_unordered_install_keeps_legacy_diagnostics() {
        let unordered_authority = authority(SCHEMA, "python-legacy-diagnostics.yaml");
        let projection = python_projection(&unordered_authority);
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();

        let typescript = TypeScriptEmitter::new();
        let foreign_target = project(
            unordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &typescript.generator_handlers(),
            &typescript.code_resources().unwrap(),
        )
        .unwrap();
        let foreign_json = String::from_utf8(to_canonical_json(&foreign_target).unwrap()).unwrap();
        let foreign_semantic =
            String::from_utf8(to_canonical_json(foreign_target.semantic_fingerprint()).unwrap())
                .unwrap();
        let foreign_fingerprint =
            String::from_utf8(to_canonical_json(foreign_target.projection_fingerprint()).unwrap())
                .unwrap();

        Python::initialize();
        Python::attach(|py| {
            let malformed = install_projection(py, "{", &semantic, &fingerprint, vec![], None)
                .err()
                .expect("malformed legacy projection must fail");
            let malformed_value = malformed.value(py);
            assert!(malformed_value.is_instance_of::<pyo3::exceptions::PyValueError>());
            assert_eq!(
                malformed_value.str().unwrap().to_str().unwrap(),
                "invalid_contract [malformed_canonical_json]: input is not valid canonical JSON",
            );
            assert!(malformed_value.getattr("sdk_category").is_err());

            let target = install_projection(
                py,
                &foreign_json,
                &foreign_semantic,
                &foreign_fingerprint,
                vec![],
                None,
            )
            .err()
            .expect("foreign legacy target must fail");
            let target_value = target.value(py);
            assert!(target_value.is_instance_of::<pyo3::exceptions::PyRuntimeError>());
            assert_eq!(
                target_value.str().unwrap().to_str().unwrap(),
                "runtime projection does not target Python",
            );
            assert!(target_value.getattr("sdk_category").is_err());

            let coverage =
                install_projection(py, &projection_json, &semantic, &fingerprint, vec![], None)
                    .err()
                    .expect("incomplete legacy registrations must fail");
            let coverage_value = coverage.value(py);
            assert!(coverage_value.is_instance_of::<pyo3::exceptions::PyValueError>());
            assert_eq!(
                coverage_value.str().unwrap().to_str().unwrap(),
                format!(
                    "projection requires exactly {} model registrations, received 0",
                    projection.models().len(),
                ),
            );
            assert!(coverage_value.getattr("sdk_category").is_err());
        });
    }

    #[test]
    fn native_lowering_and_hydration_preserve_wrappers_iids_and_relation_references() {
        Python::initialize();
        Python::attach(|py| {
            let (_, package) = install(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let identifier_id = package
                .type_by_label("identifier", TypeKind::Attribute)
                .unwrap()
                .clone();
            let aliases_id = package
                .type_by_label("aliases", TypeKind::Attribute)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let identifier_class = package
                .class(&identifier_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let aliases_class = package
                .class(&aliases_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let identifier = identifier_class.call1(("person-1",)).unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("identifier", &identifier).unwrap();
            let person = person_class.call((), Some(&kwargs)).unwrap();
            let descriptor = package.projection.entity_descriptor(&person_id).unwrap();
            assert_eq!(
                lower_attributes(py, package.as_ref(), &descriptor.owned_attributes, &person)
                    .unwrap(),
                vec![(
                    "identifier".into(),
                    AttributeValue::String("person-1".into())
                )]
            );

            let hydrated = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0x-person".into()),
                    type_name: Some("person".into()),
                    attributes: vec![(
                        "identifier".into(),
                        AttributeValue::String("person-1".into()),
                    )],
                },
            )
            .unwrap();
            let hydrated = hydrated.bind(py);
            assert_eq!(
                hydrated
                    .getattr("iid")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0x-person"
            );
            let wrapped = hydrated
                .call_method0("runtime_values")
                .unwrap()
                .cast::<PyDict>()
                .unwrap()
                .get_item("identifier")
                .unwrap()
                .unwrap();
            assert_eq!(wrapped.get_type().as_ptr(), identifier_class.as_ptr());
            assert_eq!(
                wrapped
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );

            let membership_id = package
                .type_by_label("membership", TypeKind::Relation)
                .unwrap()
                .clone();
            let membership = hydrate_relation(
                py,
                package.as_ref(),
                &membership_id,
                &DynamicRelationRow {
                    iid: Some("0x-membership".into()),
                    type_name: Some("membership".into()),
                    attributes: vec![],
                    role_players: vec![DynamicRolePlayer {
                        role_name: "member".into(),
                        player_iid: Some("0x-person".into()),
                        player_type_name: Some("person".into()),
                        attributes: vec![
                            ("identifier".into(), serde_json::json!("person-1")),
                            ("aliases".into(), serde_json::json!(["alpha", "beta"])),
                        ],
                    }],
                },
            )
            .unwrap();
            let membership_values = membership.bind(py).call_method0("runtime_values").unwrap();
            let member = membership_values
                .cast::<PyDict>()
                .unwrap()
                .get_item("member")
                .unwrap()
                .unwrap();
            assert_eq!(member.get_type().as_ptr(), person_class.as_ptr());
            assert_eq!(
                member.getattr("iid").unwrap().extract::<String>().unwrap(),
                "0x-person"
            );
            let member_values = member.call_method0("runtime_values").unwrap();
            let member_values = member_values.cast::<PyDict>().unwrap();
            let member_identifier = member_values.get_item("identifier").unwrap().unwrap();
            assert_eq!(
                member_identifier.get_type().as_ptr(),
                identifier_class.as_ptr()
            );
            assert_eq!(
                member_identifier
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );
            let member_aliases = member_values.get_item("aliases").unwrap().unwrap();
            let member_aliases = member_aliases.cast::<PyTuple>().unwrap();
            assert_eq!(member_aliases.len(), 2);
            for alias in member_aliases.iter() {
                assert_eq!(alias.get_type().as_ptr(), aliases_class.as_ptr());
            }

            let container_id = package
                .type_by_label("container", TypeKind::Relation)
                .unwrap()
                .clone();
            let relation = hydrate_relation(
                py,
                package.as_ref(),
                &container_id,
                &DynamicRelationRow {
                    iid: Some("0x-container".into()),
                    type_name: Some("container".into()),
                    attributes: vec![],
                    role_players: vec![DynamicRolePlayer {
                        role_name: "item".into(),
                        player_iid: Some("0x-event".into()),
                        player_type_name: Some("event".into()),
                        attributes: vec![],
                    }],
                },
            )
            .unwrap();
            let values = relation.bind(py).call_method0("runtime_values").unwrap();
            let item = values
                .cast::<PyDict>()
                .unwrap()
                .get_item("item")
                .unwrap()
                .unwrap();
            let item = item.cast::<PyTuple>().unwrap().get_item(0).unwrap();
            assert_eq!(
                item.getattr("__model_form__")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference"
            );
            assert_eq!(
                item.getattr("iid").unwrap().extract::<String>().unwrap(),
                "0x-event"
            );
        });
    }
}
