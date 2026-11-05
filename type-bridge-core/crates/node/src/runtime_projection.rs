//! Verified package-scoped runtime projections for generated TypeScript models.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_char, c_void};
use std::fmt::{self, Write as _};
use std::ptr;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{
    Array, BigInt, Buffer, Env, External, FromNapiRef, FromNapiValue, Function, FunctionRef,
    JsValue, Object, Unknown, type_tag_from_ident,
};
use napi::{Error, Status, sys};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{AttributeId, Iid, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH,
    MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projected_record::{
    MAX_PROJECTED_ARCHIVE_BYTES, MAX_PROJECTED_ARCHIVE_RECORDS, MAX_PROJECTED_DECODED_WEIGHT,
    ProjectedRecordKind,
};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedContainer, ProjectedModelForm, ProjectedModelUse,
    ProjectedMultiplicity, ProjectedTokenIdentity, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS, SdkDiagnosticCategory, SdkDiagnosticCode,
    SdkDiagnosticMessage, SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
    SdkProjectionEvidenceSlotPresence,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{CanonicalValue, DecimalValue, ValueTypeTag};
use type_bridge_orm::_descriptor::{OwnedAttributeDescriptor, RoleDescriptor, TypeDescriptor};
use type_bridge_orm::_dynamic::{
    DynamicAttributeMap, DynamicComparisonOp, DynamicEntityRow, DynamicExpr, DynamicRelationRow,
    DynamicRolePlayer,
};
use type_bridge_orm::projected_batch::ProjectedBatchBindingBudget;
use type_bridge_orm::{
    AnswerCancellation, AttributeValue, Database, HydratedAttribute, InstalledRuntimeProjection,
    ManagerLookup, OwnedQueryTarget, ProjectedAttributeValue, ProjectedBatch,
    ProjectedBatchExecutor, ProjectedBatchInvocationControl, ProjectedBatchOperation,
    ProjectedBatchResult, ProjectedBatchRow, ProjectedCodecValue, ProjectedCreate,
    ProjectedCrudExecutor, ProjectedManagerComparison, ProjectedManagerFilter,
    ProjectedManagerFilterExecutor, ProjectedManagerFilterInvocationControl, ProjectedReference,
    ProjectedRolePlayer, ProjectedStructValue, ProjectedThing, ProviderRuntimeOwner,
    QueryExecutionResourceLimits, QueryTarget, ThingKind, TransactionContextState, ValueType,
    resolve_generated_manager_lookup, validate_projected_create_member_count,
    validate_projected_hydrated_member_count,
};
use type_bridge_schema::VerifiedSchemaAuthority;
use type_bridge_schema::{decode_schema_authority, schema_authority_capability_vocabulary};
use type_bridge_schema_codegen::{TypeScriptEmitter, verify_projection_evidence};

use crate::match_runtime::{
    NodeQueryCancellation, NodeQueryExecutionResources, NodeQueryLimits, napi_sdk_diagnostic,
    parse_enum, revalidate_diagnostic,
};
use crate::{
    NodeMatchSessionHandle, NodeRustDatabase, NodeRustTransactionContext, NodeValidatedThingHandle,
};

struct NodeCanonicalControl {
    cancellation: AnswerCancellation,
    deadline: Option<Instant>,
    max_input_bytes: usize,
    max_output_bytes: usize,
    max_depth: usize,
    max_records: usize,
    max_members: usize,
}

impl NodeCanonicalControl {
    #[allow(clippy::too_many_arguments)]
    fn capture(
        cancellation: Option<&NodeQueryCancellation>,
        timeout_milliseconds: Option<u32>,
        max_input_bytes: Option<u32>,
        max_output_bytes: Option<u32>,
        max_depth: Option<u32>,
        max_records: Option<u32>,
        max_members: Option<u32>,
    ) -> napi::Result<Self> {
        let deadline = timeout_milliseconds
            .map(|milliseconds| {
                Instant::now()
                    .checked_add(Duration::from_millis(u64::from(milliseconds)))
                    .ok_or_else(|| {
                        napi_sdk_diagnostic(
                            SdkExecutionDiagnostic::projected_codec_deadline_exceeded(),
                        )
                    })
            })
            .transpose()?;
        Ok(Self {
            cancellation: cancellation.map_or_else(AnswerCancellation::default, |v| v.inner()),
            deadline,
            max_input_bytes: usize::try_from(max_input_bytes.unwrap_or(u32::MAX))
                .unwrap_or(usize::MAX)
                .min(MAX_PROJECTED_ARCHIVE_BYTES),
            max_output_bytes: usize::try_from(max_output_bytes.unwrap_or(u32::MAX))
                .unwrap_or(usize::MAX)
                .min(MAX_PROJECTED_ARCHIVE_BYTES),
            max_depth: usize::try_from(max_depth.unwrap_or(u32::MAX))
                .unwrap_or(usize::MAX)
                .min(MAX_CANONICAL_DEPTH),
            max_records: usize::try_from(max_records.unwrap_or(u32::MAX))
                .unwrap_or(usize::MAX)
                .min(MAX_PROJECTED_ARCHIVE_RECORDS),
            max_members: usize::try_from(max_members.unwrap_or(u32::MAX))
                .unwrap_or(usize::MAX)
                .min(MAX_PROJECTED_DECODED_WEIGHT)
                .min(MAX_CANONICAL_COLLECTION_LEN),
        })
    }

    fn check(&self) -> napi::Result<()> {
        if self.cancellation.is_cancelled() {
            Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_cancelled(),
            ))
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(napi_sdk_diagnostic(
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ModelRegistration {
    type_key: String,
    target_name: String,
    create: bool,
    reference: bool,
}

struct InstalledPackage {
    projection: Arc<InstalledRuntimeProjection>,
    managed_scope_id: Option<ManagedScopeId>,
    types_by_label: BTreeMap<String, TypeId>,
    projected_type_keys: BTreeMap<TypeId, ProjectedTypeKeyCache>,
    projected_batch_materializer: Option<Arc<FunctionRef<(), ()>>>,
}

struct ProjectedTypeKeyCache {
    canonical: String,
    json_string: String,
    roles: ProjectedRoleIndex,
}

struct ProjectedRoleIndex {
    role_ids: Vec<RoleId>,
    role_ordinals_by_target: BTreeMap<String, u32>,
}

impl ProjectedRoleIndex {
    fn role_ordinal(&self, target_name: &str) -> Option<u32> {
        self.role_ordinals_by_target.get(target_name).copied()
    }

    fn role_id(&self, ordinal: u32) -> Option<&RoleId> {
        self.role_ids.get(ordinal as usize)
    }
}

const BATCH_AUTHORITY_PENDING: u8 = 0;
const BATCH_AUTHORITY_ACTIVE: u8 = 1;
const BATCH_AUTHORITY_ABORTED: u8 = 2;

const PROJECTED_BATCH_AUTHORITY_TAG: sys::napi_type_tag =
    type_tag_from_ident("type_bridge_node::runtime_projection::NodeProjectedBatchAuthority");

/// Opaque native authority shared by every proof produced for one batch.
///
/// The authority intentionally owns only common projection authority and the
/// hydrated rows. It must not retain [`InstalledPackage`], whose stored
/// [`FunctionRef`] would otherwise remain live with every generated facade.
struct NodeProjectedBatchAuthority {
    projection: Arc<InstalledRuntimeProjection>,
    state: AtomicU8,
    rows: OnceLock<Vec<ProjectedThing>>,
}

impl NodeProjectedBatchAuthority {
    fn pending(projection: Arc<InstalledRuntimeProjection>) -> Arc<Self> {
        Arc::new(Self {
            projection,
            state: AtomicU8::new(BATCH_AUTHORITY_PENDING),
            rows: OnceLock::new(),
        })
    }

    fn is_active(&self) -> bool {
        self.state.load(Ordering::Acquire) == BATCH_AUTHORITY_ACTIVE
    }

    fn abort(&self) {
        let _ = self.state.compare_exchange(
            BATCH_AUTHORITY_PENDING,
            BATCH_AUTHORITY_ABORTED,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

struct PendingBatchAuthority {
    authority: Arc<NodeProjectedBatchAuthority>,
    armed: bool,
}

impl PendingBatchAuthority {
    fn new(authority: Arc<NodeProjectedBatchAuthority>) -> Self {
        Self {
            authority,
            armed: true,
        }
    }

    fn finish(mut self) {
        self.authority
            .state
            .store(BATCH_AUTHORITY_ACTIVE, Ordering::Release);
        self.armed = false;
    }
}

impl Drop for PendingBatchAuthority {
    fn drop(&mut self) {
        if self.armed {
            self.authority.abort();
        }
    }
}

unsafe extern "C" fn finalize_projected_batch_authority(
    _env: sys::napi_env,
    data: *mut c_void,
    _hint: *mut c_void,
) {
    if !data.is_null() {
        // SAFETY: `create_projected_batch_authority_external` installs exactly
        // one Box<Arc<_>> as this external's finalizer payload.
        drop(unsafe { Box::from_raw(data.cast::<Arc<NodeProjectedBatchAuthority>>()) });
    }
}

fn napi_status(status: sys::napi_status, message: &'static str) -> napi::Result<()> {
    if status == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(Error::new(Status::from(status), message))
    }
}

fn napi_status_preserving_exception(
    env: sys::napi_env,
    status: sys::napi_status,
    message: &'static str,
) -> napi::Result<()> {
    if status == sys::Status::napi_ok {
        return Ok(());
    }
    let mut pending = false;
    if unsafe { sys::napi_is_exception_pending(env, &mut pending) } == sys::Status::napi_ok
        && pending
    {
        let mut exception = ptr::null_mut();
        if unsafe { sys::napi_get_and_clear_last_exception(env, &mut exception) }
            == sys::Status::napi_ok
            && !exception.is_null()
        {
            // SAFETY: `exception` is a live local handle in this native call.
            let value = unsafe { Unknown::from_napi_value(env, exception) }?;
            return Err(Error::from_unknown_without_coercion(value));
        }
    }
    Err(Error::new(Status::from(status), message))
}

fn raw_typeof(env: sys::napi_env, value: sys::napi_value) -> napi::Result<sys::napi_valuetype> {
    let mut value_type = sys::ValueType::napi_undefined;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_typeof(env, value, &mut value_type) },
        "failed to inspect JavaScript value",
    )?;
    Ok(value_type)
}

fn raw_named_property(
    env: sys::napi_env,
    object: sys::napi_value,
    name: &'static std::ffi::CStr,
) -> napi::Result<sys::napi_value> {
    let mut value = ptr::null_mut();
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_named_property(env, object, name.as_ptr(), &mut value) },
        "failed to read projected batch row property",
    )?;
    Ok(value)
}

enum RawProjectedBindingError {
    Napi(Error),
    Diagnostic(SdkExecutionDiagnostic),
}

impl From<Error> for RawProjectedBindingError {
    fn from(error: Error) -> Self {
        Self::Napi(error)
    }
}

impl RawProjectedBindingError {
    fn into_napi(self) -> Error {
        match self {
            Self::Napi(error) => error,
            Self::Diagnostic(diagnostic) => napi_sdk_diagnostic(diagnostic),
        }
    }
}

fn raw_projected_binding_or_shape(
    error: RawProjectedBindingError,
    shape: impl FnOnce() -> SdkExecutionDiagnostic,
) -> Error {
    match error {
        RawProjectedBindingError::Napi(_) => napi_sdk_diagnostic(shape()),
        RawProjectedBindingError::Diagnostic(diagnostic) => napi_sdk_diagnostic(diagnostic),
    }
}

#[cfg(test)]
static FAIL_PROJECTED_BATCH_INGRESS_RESERVATION: AtomicBool = AtomicBool::new(false);

fn reserved_projected_binding_bytes(capacity: usize) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
    #[cfg(test)]
    if FAIL_PROJECTED_BATCH_INGRESS_RESERVATION.swap(false, Ordering::Relaxed) {
        return Err(ProjectedBatch::binding_allocation_failure());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
    Ok(bytes)
}

fn raw_utf8_string(
    env: sys::napi_env,
    value: sys::napi_value,
    maximum_bytes: usize,
    mismatch: &'static str,
) -> Result<String, RawProjectedBindingError> {
    if raw_typeof(env, value)? != sys::ValueType::napi_string {
        return Err(invalid_error(mismatch).into());
    }
    let mut length = 0_usize;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_value_string_utf8(env, value, ptr::null_mut(), 0, &mut length) },
        "failed to measure projected batch string",
    )?;
    if length > maximum_bytes {
        return Err(invalid_error("projected batch string exceeds its binding ceiling").into());
    }
    let capacity = length
        .checked_add(1)
        .ok_or_else(ProjectedBatch::binding_allocation_failure)
        .map_err(RawProjectedBindingError::Diagnostic)?;
    let mut bytes =
        reserved_projected_binding_bytes(capacity).map_err(RawProjectedBindingError::Diagnostic)?;
    bytes.resize(capacity, 0);
    let mut written = 0_usize;
    napi_status_preserving_exception(
        env,
        unsafe {
            sys::napi_get_value_string_utf8(
                env,
                value,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut written,
            )
        },
        "failed to copy projected batch string",
    )?;
    bytes.truncate(written);
    String::from_utf8(bytes).map_err(|_| invalid_error(mismatch).into())
}

fn raw_fixed_string_equals(
    env: sys::napi_env,
    value: sys::napi_value,
    expected: &str,
) -> napi::Result<bool> {
    if raw_typeof(env, value)? != sys::ValueType::napi_string {
        return Ok(false);
    }
    let mut length = 0_usize;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_value_string_utf8(env, value, ptr::null_mut(), 0, &mut length) },
        "failed to measure projected proof key",
    )?;
    if length != expected.len() {
        return Ok(false);
    }
    let mut bytes = [0_u8; 32];
    if length.saturating_add(1) > bytes.len() {
        return Ok(false);
    }
    let mut written = 0_usize;
    napi_status_preserving_exception(
        env,
        unsafe {
            sys::napi_get_value_string_utf8(
                env,
                value,
                bytes.as_mut_ptr().cast(),
                length.saturating_add(1),
                &mut written,
            )
        },
        "failed to copy projected proof key",
    )?;
    Ok(written == expected.len() && &bytes[..written] == expected.as_bytes())
}

fn raw_exact_own_properties(
    env: sys::napi_env,
    object: sys::napi_value,
    expected: &[&str],
) -> napi::Result<bool> {
    if raw_typeof(env, object)? != sys::ValueType::napi_object {
        return Ok(false);
    }
    let mut keys = ptr::null_mut();
    napi_status_preserving_exception(
        env,
        unsafe {
            sys::napi_get_all_property_names(
                env,
                object,
                sys::KeyCollectionMode::own_only,
                sys::KeyFilter::all_properties,
                sys::KeyConversion::numbers_to_strings,
                &mut keys,
            )
        },
        "failed to inspect projected batch row properties",
    )?;
    let mut length = 0_u32;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_array_length(env, keys, &mut length) },
        "failed to inspect projected batch row property count",
    )?;
    if usize::try_from(length).ok() != Some(expected.len()) {
        return Ok(false);
    }
    let mut seen = 0_u32;
    for index in 0..length {
        let mut key = ptr::null_mut();
        napi_status_preserving_exception(
            env,
            unsafe { sys::napi_get_element(env, keys, index, &mut key) },
            "failed to inspect projected batch row property",
        )?;
        let mut matched = None;
        for (expected_index, expected) in expected.iter().enumerate() {
            if raw_fixed_string_equals(env, key, expected)? {
                matched = Some(expected_index);
                break;
            }
        }
        let Some(matched) = matched else {
            return Ok(false);
        };
        let bit = 1_u32.checked_shl(matched as u32).unwrap_or(0);
        if bit == 0 || seen & bit != 0 {
            return Ok(false);
        }
        seen |= bit;
    }
    Ok(seen.count_ones() as usize == expected.len())
}

fn raw_u32(env: sys::napi_env, value: sys::napi_value) -> napi::Result<u32> {
    if raw_typeof(env, value)? != sys::ValueType::napi_number {
        return Err(invalid_error("projected proof index is not a number"));
    }
    let mut number = 0_f64;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_value_double(env, value, &mut number) },
        "failed to read projected proof index",
    )?;
    if !number.is_finite()
        || number.fract() != 0.0
        || !(0.0..=f64::from(u32::MAX)).contains(&number)
    {
        return Err(invalid_error(
            "projected proof index is outside the uint32 domain",
        ));
    }
    Ok(number as u32)
}

fn raw_is_null(env: sys::napi_env, value: sys::napi_value) -> napi::Result<bool> {
    let mut null = ptr::null_mut();
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_null(env, &mut null) },
        "failed to obtain JavaScript null",
    )?;
    let mut equal = false;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_strict_equals(env, value, null, &mut equal) },
        "failed to inspect JavaScript null",
    )?;
    Ok(equal)
}

fn raw_projected_input_proof(
    env: sys::napi_env,
    value: sys::napi_value,
    package: &InstalledPackage,
) -> Result<Option<NodeProjectedInputProof>, RawProjectedBindingError> {
    if raw_is_null(env, value)? {
        return Ok(None);
    }
    if raw_typeof(env, value)? == sys::ValueType::napi_external {
        // SAFETY: napi-rs checks its own Rust TypeId tag before returning the
        // external reference. JS retains the external for this native frame.
        let proof = unsafe { External::<NodeProjectedFacadeProof>::from_napi_ref(env, value) }?;
        return Ok(Some(NodeProjectedInputProof::Single(proof.proof.clone())));
    }
    if raw_typeof(env, value)? != sys::ValueType::napi_object {
        return Err(invalid_error(
            "projected proof must be an opaque single proof or batch descriptor",
        )
        .into());
    }
    let kind = raw_named_property(env, value, c"kind")?;
    let role_request = if raw_fixed_string_equals(env, kind, "root")? {
        if !raw_exact_own_properties(env, value, &["kind", "authority", "row"])? {
            return Err(
                invalid_error("projected batch root proof has an invalid property set").into(),
            );
        }
        None
    } else if raw_fixed_string_equals(env, kind, "role")? {
        if !raw_exact_own_properties(
            env,
            value,
            &["kind", "authority", "row", "roleName", "playerIndex"],
        )? {
            return Err(
                invalid_error("projected batch role proof has an invalid property set").into(),
            );
        }
        let role_name = raw_utf8_string(
            env,
            raw_named_property(env, value, c"roleName")?,
            MAX_CANONICAL_STRING_BYTES,
            "projected batch role proof name is not a string",
        )?;
        let player_index = raw_u32(env, raw_named_property(env, value, c"playerIndex")?)?;
        Some((role_name, player_index))
    } else {
        return Err(invalid_error("projected batch proof has an invalid discriminant").into());
    };
    let authority = projected_batch_authority_from_external(
        env,
        raw_named_property(env, value, c"authority")?,
    )?;
    if !authority.is_active() || !Arc::ptr_eq(&authority.projection, &package.projection) {
        return Err(invalid_error(
            "projected batch proof is not active for this installed package",
        )
        .into());
    }
    let row = raw_u32(env, raw_named_property(env, value, c"row")?)?;
    let rows = authority
        .rows
        .get()
        .ok_or_else(|| invalid_error("projected batch proof has no hydrated rows"))?;
    let thing = rows
        .get(row as usize)
        .ok_or_else(|| invalid_error("projected batch proof row is out of bounds"))?;
    let selector = if let Some((role_name, player_index)) = role_request {
        let roles = package
            .projected_type_keys
            .get(thing.type_id())
            .ok_or_else(|| invalid_error("projected batch proof model is absent"))?;
        let role_ordinal = roles
            .roles
            .role_ordinal(&role_name)
            .ok_or_else(|| invalid_error("projected batch proof role is out of bounds"))?;
        let role_id = roles
            .roles
            .role_id(role_ordinal)
            .ok_or_else(|| invalid_error("projected batch proof role is out of bounds"))?;
        if thing
            .roles()
            .get(role_id)
            .and_then(|players| players.get(player_index as usize))
            .is_none()
        {
            return Err(invalid_error("projected batch proof player is out of bounds").into());
        }
        NodeProjectedBatchProofSelector::Role {
            role_ordinal,
            player_index,
        }
    } else {
        NodeProjectedBatchProofSelector::Root
    };
    Ok(Some(NodeProjectedInputProof::Batch(
        NodeProjectedBatchProof {
            authority,
            row,
            selector,
        },
    )))
}

fn reserved_projected_proof_slots(
    capacity: usize,
) -> Result<Vec<Option<NodeProjectedInputProof>>, SdkExecutionDiagnostic> {
    #[cfg(test)]
    if FAIL_PROJECTED_BATCH_INGRESS_RESERVATION.swap(false, Ordering::Relaxed) {
        return Err(ProjectedBatch::binding_allocation_failure());
    }
    let mut proofs = Vec::new();
    proofs
        .try_reserve_exact(capacity)
        .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
    Ok(proofs)
}

fn raw_projected_input_proofs(
    env: sys::napi_env,
    value: sys::napi_value,
    package: &InstalledPackage,
    checkpoint: Option<&ProjectedBatchBindingBudget<'_>>,
) -> Result<Vec<Option<NodeProjectedInputProof>>, RawProjectedBindingError> {
    let mut is_array = false;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_is_array(env, value, &mut is_array) },
        "failed to inspect projected proof array",
    )?;
    if !is_array {
        return Err(invalid_error("projected proofs must be an exact array").into());
    }
    let mut length = 0_u32;
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_array_length(env, value, &mut length) },
        "failed to inspect projected proof array length",
    )?;
    ProjectedBatch::validate_binding_proof_count(length as usize)
        .map_err(RawProjectedBindingError::Diagnostic)?;
    let mut proofs = reserved_projected_proof_slots(length as usize)
        .map_err(RawProjectedBindingError::Diagnostic)?;
    for index in 0..length {
        if let Some(checkpoint) = checkpoint {
            checkpoint
                .checkpoint()
                .map_err(RawProjectedBindingError::Diagnostic)?;
        }
        // Exact-property enumeration creates one temporary JavaScript key
        // array. Bound its lifetime to this descriptor rather than retaining
        // up to the full proof-array ceiling in the enclosing row scope.
        let scope = RawHandleScope::open(env)?;
        let mut proof = ptr::null_mut();
        napi_status_preserving_exception(
            env,
            unsafe { sys::napi_get_element(env, value, index, &mut proof) },
            "failed to read projected proof array element",
        )?;
        let proof = raw_projected_input_proof(env, proof, package)?;
        scope.close()?;
        proofs.push(proof);
    }
    Ok(proofs)
}

const MAX_PROJECTED_BINDING_JSON_BYTES: usize =
    MAX_CANONICAL_BYTES * 8 + MAX_CANONICAL_COLLECTION_LEN * 256;

#[cfg(test)]
static FAIL_PROJECTED_BATCH_ROW_RESERVATION: AtomicBool = AtomicBool::new(false);

fn reserved_projected_batch_rows<T>(capacity: usize) -> Result<Vec<T>, SdkExecutionDiagnostic> {
    #[cfg(test)]
    if FAIL_PROJECTED_BATCH_ROW_RESERVATION.swap(false, Ordering::Relaxed) {
        return Err(ProjectedBatch::binding_allocation_failure());
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(capacity)
        .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
    Ok(rows)
}

struct RawHandleScope {
    env: sys::napi_env,
    scope: sys::napi_handle_scope,
    closed: bool,
}

impl RawHandleScope {
    fn open(env: sys::napi_env) -> napi::Result<Self> {
        let mut scope = ptr::null_mut();
        napi_status_preserving_exception(
            env,
            unsafe { sys::napi_open_handle_scope(env, &mut scope) },
            "failed to open projected batch handle scope",
        )?;
        Ok(Self {
            env,
            scope,
            closed: false,
        })
    }

    fn close(mut self) -> napi::Result<()> {
        let status = unsafe { sys::napi_close_handle_scope(self.env, self.scope) };
        self.closed = true;
        napi_status_preserving_exception(
            self.env,
            status,
            "failed to close projected batch handle scope",
        )
    }
}

impl Drop for RawHandleScope {
    fn drop(&mut self) {
        if !self.closed {
            let _ = unsafe { sys::napi_close_handle_scope(self.env, self.scope) };
        }
    }
}

fn raw_call_one(
    env: sys::napi_env,
    function: sys::napi_value,
    receiver: sys::napi_value,
    argument: sys::napi_value,
) -> napi::Result<sys::napi_value> {
    let arguments = [argument];
    let mut result = ptr::null_mut();
    napi_status_preserving_exception(
        env,
        unsafe {
            sys::napi_call_function(
                env,
                receiver,
                function,
                arguments.len(),
                arguments.as_ptr(),
                &mut result,
            )
        },
        "projected batch row callback failed",
    )?;
    Ok(result)
}

fn raw_batch_create_row(
    env: sys::napi_env,
    value: sys::napi_value,
    package: &InstalledPackage,
    checkpoint: &ProjectedBatchBindingBudget<'_>,
    update: bool,
    ordinal: usize,
) -> napi::Result<(Option<String>, String, Vec<Option<NodeProjectedInputProof>>)> {
    let row_path = projected_batch_row_path(ordinal);
    let expected: &[&str] = if update {
        &["iid", "instanceJson", "proofs"]
    } else {
        &["instanceJson", "proofs"]
    };
    if raw_is_null(env, value)? || !raw_exact_own_properties(env, value, expected)? {
        return Err(napi_sdk_diagnostic(batch_input_shape_diagnostic(
            "generated_model_layout_mismatch",
            "The generated batch row no longer has its installed runtime shape",
            &row_path,
        )));
    }
    let iid = update
        .then(|| {
            raw_utf8_string(
                env,
                raw_named_property(env, value, c"iid")?,
                MAX_CANONICAL_STRING_BYTES,
                "projected update batch IID is not a string",
            )
        })
        .transpose()
        .map_err(|error| {
            raw_projected_binding_or_shape(error, || {
                batch_input_shape_diagnostic(
                    "batch_iid_type_mismatch",
                    "Successor update rows require exact string IIDs",
                    &projected_batch_iid_path(ordinal),
                )
            })
        })?;
    let instance_json = raw_utf8_string(
        env,
        raw_named_property(env, value, c"instanceJson")?,
        MAX_PROJECTED_BINDING_JSON_BYTES,
        "projected batch instance JSON is not a string",
    )
    .map_err(|error| {
        raw_projected_binding_or_shape(error, || {
            batch_input_shape_diagnostic(
                "generated_model_layout_mismatch",
                "The generated batch row instance wire is not a bounded UTF-8 string",
                &row_path,
            )
        })
    })?;
    let proofs = raw_projected_input_proofs(
        env,
        raw_named_property(env, value, c"proofs")?,
        package,
        Some(checkpoint),
    )
    .map_err(|error| {
        raw_projected_binding_or_shape(error, || {
            batch_input_shape_diagnostic(
                "generated_model_layout_mismatch",
                "The generated batch row proofs do not match the installed runtime shape",
                &row_path,
            )
        })
    })?;
    Ok((iid, instance_json, proofs))
}

fn projected_batch_row_path(ordinal: usize) -> [SdkDiagnosticPathSegment; 2] {
    [
        SdkDiagnosticPathSegment::Argument(
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("rows")
                .expect("the static batch argument name is canonical"),
        ),
        SdkDiagnosticPathSegment::Index(u64::try_from(ordinal).unwrap_or(u64::MAX)),
    ]
}

fn projected_batch_iid_path(ordinal: usize) -> [SdkDiagnosticPathSegment; 3] {
    [
        projected_batch_row_path(ordinal)[0].clone(),
        projected_batch_row_path(ordinal)[1].clone(),
        SdkDiagnosticPathSegment::Argument(
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("iid")
                .expect("the static batch IID argument name is canonical"),
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

fn project_batch_create_json(
    package: &InstalledPackage,
    id: &TypeId,
    instance_json: &str,
    proofs: &[Option<NodeProjectedInputProof>],
    ordinal: usize,
) -> Result<ProjectedCreate, SdkExecutionDiagnostic> {
    let row_path = projected_batch_row_path(ordinal);
    let wire: ProjectedWire = serde_json::from_str(instance_json)
        .map_err(|_| malformed_projected_create_at(row_path.iter().cloned()))?;
    let expected = package
        .projected_type_keys
        .get(id)
        .ok_or_else(|| malformed_projected_create_at(row_path.iter().cloned()))?;
    if wire.type_key != expected.canonical
        || wire.form != WireForm::Complete
        || wire.value.is_some()
    {
        return Err(generated_token_package_mismatch_at(
            row_path.iter().cloned(),
        ));
    }
    project_create_wire_with_proofs(package, id, &wire, proofs).map_err(|diagnostic| {
        diagnostic
            .try_with_path_prefix(row_path)
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    })
}

fn capture_projected_batch(
    env: sys::napi_env,
    package: &InstalledPackage,
    model: &TypeId,
    operation: ProjectedBatchOperation,
    row_count: u32,
    row_at: sys::napi_value,
) -> napi::Result<(ProjectedBatch, ProjectedBatchInvocationControl)> {
    let row_count = row_count as usize;
    ProjectedBatch::validate_binding_row_count(row_count).map_err(napi_sdk_diagnostic)?;
    let control = ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits::default(),
        AnswerCancellation::default(),
    );
    let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        package.projection.as_ref(),
        model.clone(),
        operation,
        &control,
    )
    .map_err(napi_sdk_diagnostic)?;
    let mut rows = reserved_projected_batch_rows(row_count).map_err(napi_sdk_diagnostic)?;
    let mut receiver = ptr::null_mut();
    napi_status_preserving_exception(
        env,
        unsafe { sys::napi_get_undefined(env, &mut receiver) },
        "failed to create projected batch row callback receiver",
    )?;
    for ordinal in 0..row_count {
        budget.checkpoint().map_err(napi_sdk_diagnostic)?;
        let scope = RawHandleScope::open(env)?;
        let mut ordinal_value = ptr::null_mut();
        let ordinal_u32 = u32::try_from(ordinal)
            .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        napi_status_preserving_exception(
            env,
            unsafe { sys::napi_create_uint32(env, ordinal_u32, &mut ordinal_value) },
            "failed to create projected batch row ordinal",
        )?;
        let value = raw_call_one(env, row_at, receiver, ordinal_value)?;
        let row = match operation {
            ProjectedBatchOperation::Delete => {
                let iid = raw_utf8_string(
                    env,
                    value,
                    MAX_CANONICAL_STRING_BYTES,
                    "projected delete batch row is not a string",
                )
                .map_err(|error| {
                    raw_projected_binding_or_shape(error, || {
                        batch_input_shape_diagnostic(
                            "batch_iid_type_mismatch",
                            "Successor delete rows require exact string IIDs",
                            &projected_batch_iid_path(ordinal),
                        )
                    })
                })?;
                ProjectedBatchRow::Delete { iid }
            }
            ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put => {
                let (_, instance_json, proofs) =
                    raw_batch_create_row(env, value, package, &budget, false, ordinal)?;
                let replacement =
                    project_batch_create_json(package, model, &instance_json, &proofs, ordinal)
                        .map_err(napi_sdk_diagnostic)?;
                ProjectedBatchRow::Create(replacement)
            }
            ProjectedBatchOperation::Update => {
                let (iid, instance_json, proofs) =
                    raw_batch_create_row(env, value, package, &budget, true, ordinal)?;
                let replacement =
                    project_batch_create_json(package, model, &instance_json, &proofs, ordinal)
                        .map_err(napi_sdk_diagnostic)?;
                ProjectedBatchRow::Update {
                    iid: iid.expect("an exact update row always carries its IID"),
                    replacement,
                }
            }
        };
        scope.close()?;
        budget
            .try_add_row(package.projection.as_ref(), &row)
            .map_err(napi_sdk_diagnostic)?;
        rows.push(row);
    }
    drop(budget);
    let batch = ProjectedBatch::try_new_for_invocation(
        package.projection.as_ref(),
        model.clone(),
        operation,
        rows,
        &control,
    )
    .map_err(napi_sdk_diagnostic)?;
    Ok((batch, control))
}

fn create_projected_batch_authority_external(
    env: sys::napi_env,
    authority: &Arc<NodeProjectedBatchAuthority>,
) -> napi::Result<sys::napi_value> {
    let payload = Box::into_raw(Box::new(Arc::clone(authority)));
    let mut value = ptr::null_mut();
    let status = unsafe {
        sys::napi_create_external(
            env,
            payload.cast(),
            Some(finalize_projected_batch_authority),
            ptr::null_mut(),
            &mut value,
        )
    };
    if status != sys::Status::napi_ok {
        // SAFETY: creation failed, so Node did not assume finalizer ownership.
        drop(unsafe { Box::from_raw(payload) });
        return Err(Error::new(
            Status::from(status),
            "failed to create projected batch authority",
        ));
    }
    let status = unsafe { sys::napi_type_tag_object(env, value, &PROJECTED_BATCH_AUTHORITY_TAG) };
    napi_status(status, "failed to tag projected batch authority")?;
    Ok(value)
}

fn projected_batch_authority_from_external(
    env: sys::napi_env,
    value: sys::napi_value,
) -> napi::Result<Arc<NodeProjectedBatchAuthority>> {
    let mut value_type = sys::ValueType::napi_undefined;
    napi_status(
        unsafe { sys::napi_typeof(env, value, &mut value_type) },
        "failed to inspect projected batch authority",
    )?;
    if value_type != sys::ValueType::napi_external {
        return Err(invalid_error(
            "projected batch authority is not an external",
        ));
    }
    let mut tagged = false;
    napi_status(
        unsafe {
            sys::napi_check_object_type_tag(env, value, &PROJECTED_BATCH_AUTHORITY_TAG, &mut tagged)
        },
        "failed to authenticate projected batch authority",
    )?;
    if !tagged {
        return Err(invalid_error("projected batch authority is not authentic"));
    }
    let mut payload = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_get_value_external(env, value, &mut payload) },
        "failed to unwrap projected batch authority",
    )?;
    if payload.is_null() {
        return Err(invalid_error("projected batch authority is empty"));
    }
    // SAFETY: the exact type tag is installed only for a Box<Arc<_>> created
    // above; cloning the Arc leaves ownership with the JS external finalizer.
    Ok(Arc::clone(unsafe {
        &*payload.cast::<Arc<NodeProjectedBatchAuthority>>()
    }))
}

/// Read snapshot state from the same opaque authority used for batch publication.
#[napi(js_name = "projectedBatchDeleted")]
pub fn projected_batch_deleted(
    env: Env,
    authority: Unknown,
    ordinal: u32,
    role_name: Option<String>,
    player_index: Option<u32>,
) -> napi::Result<bool> {
    let authority = projected_batch_authority_from_external(env.raw(), authority.raw())?;
    if authority.state.load(Ordering::Acquire) == BATCH_AUTHORITY_ABORTED {
        return Err(invalid_error("projected batch publication was aborted"));
    }
    let thing = authority
        .rows
        .get()
        .and_then(|rows| rows.get(ordinal as usize))
        .ok_or_else(|| invalid_error("projected batch row is not materialized"))?;
    match (role_name, player_index) {
        (None, None) => Ok(thing.is_deleted()),
        (Some(name), Some(index)) => {
            let role = authority
                .projection
                .projection()
                .models()
                .get(thing.type_id())
                .and_then(|model| {
                    model
                        .query_tokens()
                        .roles()
                        .iter()
                        .find(|(_, role)| role.target_name().as_str() == name)
                })
                .map(|(id, _)| id)
                .ok_or_else(|| invalid_error("projected batch role is not materialized"))?;
            let player = thing
                .roles()
                .get(role)
                .and_then(|players| players.get(index as usize))
                .ok_or_else(|| invalid_error("projected batch player is not materialized"))?;
            Ok(player
                .reference()
                .origin_carrier()
                .is_some_and(|origin| origin.is_deleted()))
        }
        _ => Err(invalid_error(
            "projected batch player selector is incomplete",
        )),
    }
}

#[derive(Default)]
struct FallibleJsonWriter {
    bytes: Vec<u8>,
}

impl FallibleJsonWriter {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Write for FallibleJsonWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.bytes
            .len()
            .checked_add(value.len())
            .ok_or(fmt::Error)?;
        self.bytes
            .try_reserve(value.len())
            .map_err(|_| fmt::Error)?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

enum BindingWriteAnswers {
    Single(Vec<ProjectedThing>),
    Batch(ProjectedBatchResult),
}

impl From<ProjectedBatchResult> for BindingWriteAnswers {
    fn from(result: ProjectedBatchResult) -> Self {
        Self::Batch(result)
    }
}

struct PreparedProjectedBatchOutput {
    env: sys::napi_env,
    function: sys::napi_value,
    receiver: sys::napi_value,
    type_key: sys::napi_value,
    null: sys::napi_value,
    authority_value: sys::napi_value,
    array: sys::napi_value,
    ordinals: Vec<sys::napi_value>,
    authority: Arc<NodeProjectedBatchAuthority>,
    package: Arc<InstalledPackage>,
}

fn decimal_property_name(value: u32, output: &mut [u8; 11]) -> *const c_char {
    output.fill(0);
    let mut cursor = output.len().saturating_sub(1);
    let mut remaining = value;
    loop {
        cursor = cursor.saturating_sub(1);
        output[cursor] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    output[cursor..].as_ptr().cast()
}

fn prepare_projected_batch_output(
    env: &Env,
    package: Arc<InstalledPackage>,
    type_id: &TypeId,
    row_count: usize,
    authority: Arc<NodeProjectedBatchAuthority>,
) -> napi::Result<PreparedProjectedBatchOutput> {
    let materializer = package
        .projected_batch_materializer
        .as_ref()
        .ok_or_else(|| invalid_error("native projection has no installed batch materializer"))?;
    let function = materializer.borrow_back(env)?.raw();
    let raw_env = env.raw();
    let mut receiver = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_get_undefined(raw_env, &mut receiver) },
        "failed to create projected batch materializer receiver",
    )?;
    let type_key_cache = package
        .projected_type_keys
        .get(type_id)
        .ok_or_else(|| runtime_error("projected batch model type key is absent"))?;
    let type_key_length = isize::try_from(type_key_cache.canonical.len())
        .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    let mut type_key = ptr::null_mut();
    napi_status(
        unsafe {
            sys::napi_create_string_utf8(
                raw_env,
                type_key_cache.canonical.as_ptr().cast(),
                type_key_length,
                &mut type_key,
            )
        },
        "failed to create projected batch model type key",
    )?;
    let mut null = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_get_null(raw_env, &mut null) },
        "failed to create projected batch null sentinel",
    )?;
    let authority_value = create_projected_batch_authority_external(raw_env, &authority)?;
    let mut array = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_create_array_with_length(raw_env, row_count, &mut array) },
        "failed to create projected batch result array",
    )?;
    for ordinal in 0..row_count {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        let mut name = [0_u8; 11];
        let descriptor = sys::napi_property_descriptor {
            utf8name: decimal_property_name(ordinal, &mut name),
            name: ptr::null_mut(),
            method: None,
            getter: None,
            setter: None,
            value: receiver,
            attributes: sys::PropertyAttributes::writable
                | sys::PropertyAttributes::enumerable
                | sys::PropertyAttributes::configurable,
            data: ptr::null_mut(),
        };
        napi_status(
            unsafe { sys::napi_define_properties(raw_env, array, 1, &descriptor) },
            "failed to create projected batch own result slot",
        )?;
    }
    let mut ordinals = Vec::new();
    ordinals
        .try_reserve_exact(row_count)
        .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    for ordinal in 0..row_count {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
        let mut value = ptr::null_mut();
        napi_status(
            unsafe { sys::napi_create_uint32(raw_env, ordinal, &mut value) },
            "failed to create projected batch materializer ordinal",
        )?;
        ordinals.push(value);
    }
    Ok(PreparedProjectedBatchOutput {
        env: raw_env,
        function,
        receiver,
        type_key,
        null,
        authority_value,
        array,
        ordinals,
        authority,
        package,
    })
}

fn projected_write_result_object(
    env: sys::napi_env,
    items: sys::napi_value,
    count: usize,
    input_indexes: Option<&[usize]>,
    single: bool,
) -> napi::Result<sys::napi_value> {
    let (last_name, last_value) = if single {
        let mut item = ptr::null_mut();
        if count == 1 {
            napi_status(
                unsafe { sys::napi_get_element(env, items, 0, &mut item) },
                "failed to retain projected single result item",
            )?;
        } else {
            napi_status(
                unsafe { sys::napi_get_null(env, &mut item) },
                "failed to create projected single result sentinel",
            )?;
        }
        (c"item", item)
    } else {
        let input_indexes =
            input_indexes.ok_or_else(|| runtime_error("batch result omitted its input indexes"))?;
        if input_indexes.len() != count {
            return Err(runtime_error("batch result input index count changed"));
        }
        let mut indexes = ptr::null_mut();
        napi_status(
            unsafe { sys::napi_create_array_with_length(env, input_indexes.len(), &mut indexes) },
            "failed to create projected batch input indexes",
        )?;
        for (ordinal, index) in input_indexes.iter().copied().enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
            let mut value = ptr::null_mut();
            napi_status(
                unsafe { sys::napi_create_uint32(env, index, &mut value) },
                "failed to create projected batch input index",
            )?;
            let mut name = [0_u8; 11];
            let ordinal = u32::try_from(ordinal)
                .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
            let descriptor = sys::napi_property_descriptor {
                utf8name: decimal_property_name(ordinal, &mut name),
                name: ptr::null_mut(),
                method: None,
                getter: None,
                setter: None,
                value,
                attributes: sys::PropertyAttributes::enumerable,
                data: ptr::null_mut(),
            };
            napi_status(
                unsafe { sys::napi_define_properties(env, indexes, 1, &descriptor) },
                "failed to retain projected batch input index",
            )?;
        }
        napi_status(
            unsafe { sys::napi_object_freeze(env, indexes) },
            "failed to freeze projected batch input indexes",
        )?;
        (c"inputIndexes", indexes)
    };
    let mut count_value = ptr::null_mut();
    let length = u32::try_from(count)
        .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    napi_status(
        unsafe { sys::napi_create_uint32(env, length, &mut count_value) },
        "failed to create projected batch count",
    )?;
    let mut output = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_create_object(env, &mut output) },
        "failed to create projected batch result",
    )?;
    let descriptors = [
        (c"count", count_value),
        (c"items", items),
        (last_name, last_value),
    ]
    .map(|(name, value)| sys::napi_property_descriptor {
        utf8name: name.as_ptr(),
        name: ptr::null_mut(),
        method: None,
        getter: None,
        setter: None,
        value,
        attributes: sys::PropertyAttributes::enumerable,
        data: ptr::null_mut(),
    });
    napi_status(
        unsafe {
            sys::napi_define_properties(env, output, descriptors.len(), descriptors.as_ptr())
        },
        "failed to retain projected batch result",
    )?;
    napi_status(
        unsafe { sys::napi_object_freeze(env, output) },
        "failed to freeze projected batch result",
    )?;
    Ok(output)
}

fn clear_pending_js_exception(env: sys::napi_env) {
    let mut pending = false;
    if unsafe { sys::napi_is_exception_pending(env, &mut pending) } == sys::Status::napi_ok
        && pending
    {
        let mut exception = ptr::null_mut();
        let _ = unsafe { sys::napi_get_and_clear_last_exception(env, &mut exception) };
    }
}

fn mapper_status(env: sys::napi_env, status: sys::napi_status) -> Result<(), ()> {
    if status == sys::Status::napi_ok {
        Ok(())
    } else {
        clear_pending_js_exception(env);
        Err(())
    }
}

struct MapperHandleScope {
    env: sys::napi_env,
    scope: sys::napi_handle_scope,
    armed: bool,
}

impl MapperHandleScope {
    fn open(env: sys::napi_env) -> Result<Self, ()> {
        let mut scope = ptr::null_mut();
        mapper_status(env, unsafe { sys::napi_open_handle_scope(env, &mut scope) })?;
        Ok(Self {
            env,
            scope,
            armed: true,
        })
    }

    fn close(mut self) -> Result<(), ()> {
        let status = unsafe { sys::napi_close_handle_scope(self.env, self.scope) };
        self.armed = false;
        mapper_status(self.env, status)
    }
}

impl Drop for MapperHandleScope {
    fn drop(&mut self) {
        if self.armed {
            let _ = unsafe { sys::napi_close_handle_scope(self.env, self.scope) };
        }
    }
}

fn materialization_failure(ordinal: usize) -> SdkExecutionDiagnostic {
    ProjectedBatchExecutor::binding_materialization_failure(
        u64::try_from(ordinal).unwrap_or(u64::MAX),
    )
}

impl PreparedProjectedBatchOutput {
    fn materialize(
        self,
        result: BindingWriteAnswers,
        single: bool,
    ) -> Result<sys::napi_value, SdkExecutionDiagnostic> {
        let (things, input_indexes) = match result {
            BindingWriteAnswers::Single(things) => (things, None),
            BindingWriteAnswers::Batch(ProjectedBatchResult::Things(result)) => {
                let (things, indexes) = result.into_parts();
                (things, Some(indexes))
            }
            _ => return Err(SdkExecutionDiagnostic::internal_failure()),
        };
        if things.len() != self.ordinals.len() {
            return Err(materialization_failure(0));
        }
        if self.authority.state.load(Ordering::Relaxed) != BATCH_AUTHORITY_PENDING {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        self.authority
            .rows
            .set(things)
            .map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
        let rows = self
            .authority
            .rows
            .get()
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let mut json = FallibleJsonWriter::default();
        for (ordinal, thing) in rows.iter().enumerate() {
            let input_index = input_indexes.as_ref().map_or(0, |indexes| indexes[ordinal]);
            json.clear();
            write_projected_thing_json(&mut json, self.package.as_ref(), thing)
                .map_err(|_| materialization_failure(input_index))?;
            let scope = MapperHandleScope::open(self.env)
                .map_err(|_| materialization_failure(input_index))?;
            let mut json_value = ptr::null_mut();
            let json_length = isize::try_from(json.as_bytes().len())
                .map_err(|_| materialization_failure(input_index))?;
            mapper_status(self.env, unsafe {
                sys::napi_create_string_utf8(
                    self.env,
                    json.as_bytes().as_ptr().cast(),
                    json_length,
                    &mut json_value,
                )
            })
            .map_err(|_| materialization_failure(input_index))?;
            let arguments = [
                self.type_key,
                self.ordinals[ordinal],
                json_value,
                self.authority_value,
            ];
            let mut facade = ptr::null_mut();
            mapper_status(self.env, unsafe {
                sys::napi_call_function(
                    self.env,
                    self.receiver,
                    self.function,
                    arguments.len(),
                    arguments.as_ptr(),
                    &mut facade,
                )
            })
            .map_err(|_| materialization_failure(input_index))?;
            let mut value_type = sys::ValueType::napi_undefined;
            mapper_status(self.env, unsafe {
                sys::napi_typeof(self.env, facade, &mut value_type)
            })
            .map_err(|_| materialization_failure(input_index))?;
            let mut is_null = false;
            mapper_status(self.env, unsafe {
                sys::napi_strict_equals(self.env, facade, self.null, &mut is_null)
            })
            .map_err(|_| materialization_failure(input_index))?;
            let mut is_array = false;
            mapper_status(self.env, unsafe {
                sys::napi_is_array(self.env, facade, &mut is_array)
            })
            .map_err(|_| materialization_failure(input_index))?;
            if value_type != sys::ValueType::napi_object || is_null || is_array {
                return Err(materialization_failure(input_index));
            }
            mapper_status(self.env, unsafe {
                sys::napi_set_element(
                    self.env,
                    self.array,
                    u32::try_from(ordinal).unwrap_or(u32::MAX),
                    facade,
                )
            })
            .map_err(|_| materialization_failure(input_index))?;
            scope
                .close()
                .map_err(|_| materialization_failure(input_index))?;
        }
        mapper_status(self.env, unsafe {
            sys::napi_object_freeze(self.env, self.array)
        })
        .map_err(|_| materialization_failure(0))?;
        projected_write_result_object(
            self.env,
            self.array,
            self.ordinals.len(),
            input_indexes.as_deref(),
            single,
        )
        .map_err(|_| {
            clear_pending_js_exception(self.env);
            materialization_failure(0)
        })
    }
}

fn frozen_empty_array<'env>(env: &Env) -> napi::Result<Array<'env>> {
    let mut array = ptr::null_mut();
    napi_status(
        unsafe { sys::napi_create_array_with_length(env.raw(), 0, &mut array) },
        "failed to create empty projected batch result",
    )?;
    napi_status(
        unsafe { sys::napi_object_freeze(env.raw(), array) },
        "failed to freeze empty projected batch result",
    )?;
    // SAFETY: `array` is a live Array local in `env`'s current native scope.
    unsafe { Array::from_napi_value(env.raw(), array) }
}

#[derive(Clone)]
enum FacadeProjectionProof {
    Thing(Arc<ProjectedThing>),
    DetachedSnapshot,
    Reference(Arc<ProjectedReference>),
}

impl FacadeProjectionProof {
    fn reference(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
        match self {
            Self::Thing(thing) => thing.try_to_reference(installed),
            Self::DetachedSnapshot => Err(SdkExecutionDiagnostic::projected_snapshot_detached()),
            Self::Reference(reference) => {
                reference.validate_for(installed)?;
                Ok(reference.as_ref().clone())
            }
        }
    }
}

/// Opaque native proof retained only by an ordered generated facade WeakMap.
pub struct NodeProjectedFacadeProof {
    proof: FacadeProjectionProof,
}

enum NodeProjectedBatchProofSelector {
    Root,
    Role {
        role_ordinal: u32,
        player_index: u32,
    },
}

struct NodeProjectedBatchProof {
    authority: Arc<NodeProjectedBatchAuthority>,
    row: u32,
    selector: NodeProjectedBatchProofSelector,
}

enum NodeProjectedInputProof {
    Single(FacadeProjectionProof),
    Batch(NodeProjectedBatchProof),
}

impl NodeProjectedInputProof {
    fn reference(
        &self,
        package: &InstalledPackage,
    ) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
        match self {
            Self::Single(proof) => proof.reference(package.projection.as_ref()),
            Self::Batch(proof) => {
                if !proof.authority.is_active()
                    || !Arc::ptr_eq(&proof.authority.projection, &package.projection)
                {
                    return Err(malformed_projected_create_at(std::iter::empty()));
                }
                let rows = proof
                    .authority
                    .rows
                    .get()
                    .ok_or_else(|| malformed_projected_create_at(std::iter::empty()))?;
                let row = rows
                    .get(proof.row as usize)
                    .ok_or_else(|| malformed_projected_create_at(std::iter::empty()))?;
                match &proof.selector {
                    NodeProjectedBatchProofSelector::Root => {
                        row.try_to_reference(package.projection.as_ref())
                    }
                    NodeProjectedBatchProofSelector::Role {
                        role_ordinal,
                        player_index,
                    } => {
                        let role_id = package
                            .projected_type_keys
                            .get(row.type_id())
                            .and_then(|cache| cache.roles.role_id(*role_ordinal))
                            .ok_or_else(|| malformed_projected_create_at(std::iter::empty()))?;
                        let player = row
                            .roles()
                            .get(role_id)
                            .and_then(|players| players.get(*player_index as usize))
                            .ok_or_else(|| malformed_projected_create_at(std::iter::empty()))?;
                        let retained = player.reference().clone();
                        retained.validate_for(package.projection.as_ref())?;
                        Ok(retained)
                    }
                }
            }
        }
    }
}

/// One private projected wire and its non-serializable facade proofs.
#[napi]
pub struct NodeProjectedValueEnvelope {
    package: Arc<InstalledPackage>,
    json: String,
    thing: Arc<ProjectedThing>,
}

#[napi]
impl NodeProjectedValueEnvelope {
    /// Whether the root is a pre-delete snapshot.
    #[napi(getter)]
    pub fn deleted(&self) -> bool {
        self.thing.is_deleted()
    }

    /// Return the public generated-value wire without any origin evidence.
    #[napi(getter)]
    pub fn json(&self) -> String {
        self.json.clone()
    }

    /// Clone the opaque proof for the root complete facade.
    #[napi(js_name = "rootProof")]
    pub fn root_proof(&self) -> External<NodeProjectedFacadeProof> {
        External::new(NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Thing(Arc::clone(&self.thing)),
        })
    }

    /// Clone the opaque reference proof for one exact hydrated role facade.
    #[napi(js_name = "roleProof")]
    pub fn role_proof(
        &self,
        role_name: String,
        player_index: u32,
    ) -> napi::Result<External<NodeProjectedFacadeProof>> {
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(self.thing.type_id())
            .ok_or_else(|| runtime_error("projected proof root model is absent"))?;
        let role_id = model
            .query_tokens()
            .roles()
            .iter()
            .find(|(_, role)| role.target_name().as_str() == role_name)
            .map(|(role_id, _)| role_id)
            .ok_or_else(|| runtime_error("projected proof role is absent"))?;
        let player = self
            .thing
            .roles()
            .get(role_id)
            .and_then(|players| players.get(player_index as usize))
            .ok_or_else(|| runtime_error("projected proof role player is absent"))?;
        Ok(External::new(NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Reference(Arc::new(player.reference().clone())),
        }))
    }
}

impl InstalledPackage {
    fn type_by_label(&self, label: &str) -> napi::Result<&TypeId> {
        self.types_by_label
            .get(label)
            .ok_or_else(|| runtime_error("provider row type is outside the installed projection"))
    }
}

pub(crate) fn direct_tls_from_node(
    tls_mode: &str,
    tls_root_ca: Option<String>,
) -> napi::Result<type_bridge_orm::TlsPolicy> {
    match (tls_mode, tls_root_ca) {
        ("disabled", None) => Ok(type_bridge_orm::TlsPolicy::disabled()),
        ("native_roots", None) => Ok(type_bridge_orm::TlsPolicy::native_roots()),
        ("custom_root", Some(path)) => {
            type_bridge_orm::TlsPolicy::custom_root(path).map_err(napi_sdk_diagnostic)
        }
        ("custom_root", None) => Err(invalid_error("custom_root TLS requires tlsRootCa")),
        ("disabled" | "native_roots", Some(_)) => Err(invalid_error(
            "tlsRootCa is valid only with custom_root TLS",
        )),
        _ => Err(invalid_error(
            "tlsMode must be disabled, native_roots, or custom_root",
        )),
    }
}

/// A canonical runtime projection installed for exactly one generated package.
#[napi]
pub struct NodeRuntimeProjection {
    package: Arc<InstalledPackage>,
}

/// One borrowed-proof cell marshalled to the generated host runtime.
#[napi(object, object_from_js = false)]
pub struct NodeAuthoredPlanCell {
    pub model: Option<NodeProjectedValueEnvelope>,
    pub scalar: Option<String>,
}
/// One committed authored projection; exactly one result shape is present.
#[napi(object, object_from_js = false)]
pub struct NodeAuthoredPlanResult {
    pub models: Option<Vec<NodeProjectedValueEnvelope>>,
    pub rows: Option<Vec<Vec<NodeAuthoredPlanCell>>>,
}

pub(crate) struct AuthoredPlanProjection {
    pub(crate) stack: type_bridge_orm::match_request::PlanFetchStack,
    pub(crate) registry: Arc<type_bridge_orm::_registry::DescriptorRegistry>,
    package: Arc<InstalledPackage>,
    binding: type_bridge_contract::migration_assertion::BindingId,
    declared: TypeId,
    pub(crate) bulk: bool,
    pub(crate) tuple: Option<Vec<(type_bridge_contract::migration_assertion::BindingId, TypeId)>>,
}
impl AuthoredPlanProjection {
    pub(crate) fn with_plans(
        &self,
        plans: type_bridge_contract::query_plan::v3::BuiltPlans,
    ) -> napi::Result<Self> {
        Self::prepare(
            Arc::clone(&self.package),
            Arc::clone(&self.registry),
            plans,
            self.binding,
            self.declared.clone(),
            self.tuple.clone(),
        )
    }
    fn prepare(
        package: Arc<InstalledPackage>,
        registry: Arc<type_bridge_orm::_registry::DescriptorRegistry>,
        plans: type_bridge_contract::query_plan::v3::BuiltPlans,
        binding: type_bridge_contract::migration_assertion::BindingId,
        declared: TypeId,
        tuple: Option<Vec<(type_bridge_contract::migration_assertion::BindingId, TypeId)>>,
    ) -> napi::Result<Self> {
        let descriptor = match declared.kind() {
            TypeKind::Entity => format!("entity:{}", declared.label().as_str()),
            TypeKind::Relation => format!("relation:{}", declared.label().as_str()),
            _ => {
                return Err(invalid_error(
                    "model projection requires a thing descriptor",
                ));
            }
        };
        let bulk = plans.bulk_rows().is_some();
        let stack = if let Some(slots) = &tuple {
            let [plan] = plans.plans() else {
                return Err(invalid_error("model tuples require one authored selection"));
            };
            let slots = slots
                .iter()
                .map(|(binding, id)| {
                    let kind = match id.kind() {
                        TypeKind::Entity => "entity",
                        TypeKind::Relation => "relation",
                        _ => return Err(invalid_error("model tuple requires thing descriptors")),
                    };
                    Ok((
                        *binding,
                        type_bridge_orm::match_request::DescriptorId::new(format!(
                            "{kind}:{}",
                            id.label().as_str()
                        )),
                    ))
                })
                .collect::<napi::Result<Vec<_>>>()?;
            type_bridge_orm::match_request::build_plan_fetch_stack(&registry, plan, &slots)
        } else {
            type_bridge_orm::match_request::build_bulk_plan_fetch_stack(
                &registry,
                plans,
                (
                    binding,
                    type_bridge_orm::match_request::DescriptorId::new(descriptor),
                ),
            )
        }
        .map_err(|error| crate::plan_runtime::napi_error(&error))?;
        Ok(Self {
            stack,
            registry,
            package,
            binding,
            declared,
            bulk,
            tuple,
        })
    }
    pub(crate) fn materialize_rows(
        &self,
        proof: &type_bridge_orm::match_request::OwnedPlanFetchResult,
        origin: type_bridge_orm::ProjectedQueryOrigin,
        cache: &OnceLock<Result<type_bridge_orm::ProjectedQueryValue, SdkExecutionDiagnostic>>,
    ) -> napi::Result<Vec<Vec<NodeAuthoredPlanCell>>> {
        let projected = cache
            .get_or_init(|| {
                origin
                    .materialize_borrowed_with_budget(
                        &self.package.projection,
                        &self.registry,
                        proof.request(),
                        proof.result(),
                        Default::default(),
                        &type_bridge_orm::AnswerCancellation::default(),
                        None,
                    )
                    .map(|(value, _)| value)
            })
            .as_ref()
            .map_err(|error| napi_sdk_diagnostic(error.clone()))?;
        let type_bridge_orm::ProjectedQueryValue::Rows { rows } = projected else {
            return Err(runtime_error("model tuple did not publish model rows"));
        };
        (0..proof.row_count())
            .map(|index| {
                proof
                    .columns()
                    .iter()
                    .map(|binding| match proof.cell(index, *binding) {
                        Some(type_bridge_orm::match_request::PlanFetchCell::Scalar(value)) => {
                            Ok(NodeAuthoredPlanCell {
                                model: None,
                                scalar: Some(serde_json::to_string(value).map_err(json_error)?),
                            })
                        }
                        Some(type_bridge_orm::match_request::PlanFetchCell::Model {
                            row,
                            slot,
                        }) => {
                            let slot = rows
                                .get(row)
                                .and_then(|row| row.slots().get(slot))
                                .ok_or_else(|| {
                                    runtime_error("model tuple ordinal is outside its proof")
                                })?;
                            let type_bridge_orm::ProjectedQuerySlotValue::One(thing) = slot.value()
                            else {
                                return Err(runtime_error("model tuple requires a singular model"));
                            };
                            Ok(NodeAuthoredPlanCell {
                                model: Some(projected_envelope_arc(
                                    Arc::clone(&self.package),
                                    Arc::clone(thing),
                                )?),
                                scalar: None,
                            })
                        }
                        None => Err(runtime_error("model tuple cell is outside its proof")),
                    })
                    .collect()
            })
            .collect()
    }
    pub(crate) fn materialize(
        &self,
        proof: &type_bridge_orm::match_request::OwnedPlanFetchResult,
        origin: type_bridge_orm::ProjectedQueryOrigin,
        cache: &OnceLock<Result<type_bridge_orm::ProjectedQueryValue, SdkExecutionDiagnostic>>,
    ) -> napi::Result<Vec<NodeProjectedValueEnvelope>> {
        let projected = cache
            .get_or_init(|| {
                origin
                    .materialize_borrowed_with_budget(
                        &self.package.projection,
                        &self.registry,
                        proof.request(),
                        proof.result(),
                        Default::default(),
                        &type_bridge_orm::AnswerCancellation::default(),
                        None,
                    )
                    .map(|(value, _)| value)
            })
            .as_ref()
            .map_err(|error| napi_sdk_diagnostic(error.clone()))?;
        let type_bridge_orm::ProjectedQueryValue::Rows { rows } = projected else {
            return Err(runtime_error("model projection did not publish model rows"));
        };
        proof
            .model_rows()
            .iter()
            .map(|ordinal| {
                let slot = rows
                    .get(*ordinal)
                    .and_then(|row| row.slots().first())
                    .ok_or_else(|| {
                        runtime_error("model projection ordinal is outside its proof")
                    })?;
                let type_bridge_orm::ProjectedQuerySlotValue::One(thing) = slot.value() else {
                    return Err(runtime_error(
                        "model projection did not publish a singular model",
                    ));
                };
                projected_envelope_arc(Arc::clone(&self.package), Arc::clone(thing))
            })
            .collect()
    }
}
impl NodeRuntimeProjection {
    pub(crate) fn prepare_authored_plan(
        &self,
        plans: type_bridge_contract::query_plan::v3::BuiltPlans,
        binding: type_bridge_contract::migration_assertion::BindingId,
        declared: &str,
    ) -> napi::Result<AuthoredPlanProjection> {
        let declared = type_bridge_contract::codec::from_canonical_json(declared.as_bytes())
            .map_err(|error| crate::plan_runtime::napi_error(&error))?;
        let registry = Arc::new(
            self.package
                .projection
                .match_registry()
                .map_err(orm_error)?,
        );
        AuthoredPlanProjection::prepare(
            Arc::clone(&self.package),
            registry,
            plans,
            binding,
            declared,
            None,
        )
    }
    pub(crate) fn prepare_authored_tuple(
        &self,
        plans: type_bridge_contract::query_plan::v3::BuiltPlans,
        bindings: Vec<u32>,
        declared: Vec<String>,
    ) -> napi::Result<AuthoredPlanProjection> {
        if bindings.len() != declared.len() {
            return Err(invalid_error(
                "model tuple bindings and descriptors differ in length",
            ));
        }
        let slots = bindings
            .into_iter()
            .zip(declared)
            .map(|(binding, declared)| {
                let binding = u16::try_from(binding)
                    .ok()
                    .and_then(|binding| {
                        type_bridge_contract::migration_assertion::BindingId::new(binding).ok()
                    })
                    .ok_or_else(|| invalid_error("model tuple binding is invalid"))?;
                let declared =
                    type_bridge_contract::codec::from_canonical_json(declared.as_bytes())
                        .map_err(|error| crate::plan_runtime::napi_error(&error))?;
                Ok((binding, declared))
            })
            .collect::<napi::Result<Vec<_>>>()?;
        let (binding, declared) = slots
            .first()
            .cloned()
            .ok_or_else(|| invalid_error("model tuple requires model bindings"))?;
        let registry = Arc::new(
            self.package
                .projection
                .match_registry()
                .map_err(orm_error)?,
        );
        AuthoredPlanProjection::prepare(
            Arc::clone(&self.package),
            registry,
            plans,
            binding,
            declared,
            Some(slots),
        )
    }
}

enum BatchMaterializerPolicy {
    RequiredSuccessor,
    OptionalSuccessor,
    PlanResultsOnly,
}

/// Install the same mapper for ordinary plan results, without enabling legacy CRUD batches.
#[napi(js_name = "installPlanRuntimeProjection")]
pub fn install_plan_runtime_projection(
    projection_json: String,
    semantic_fingerprint_json: String,
    projection_fingerprint_json: String,
    registrations_json: String,
    schema_authority_json: String,
    projected_batch_materializer: Function<'_, (), ()>,
) -> napi::Result<NodeRuntimeProjection> {
    NodeRuntimeProjection::install_with_policy(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        registrations_json,
        Some(schema_authority_json),
        Some(projected_batch_materializer),
        BatchMaterializerPolicy::PlanResultsOnly,
    )
}

#[napi]
impl NodeRuntimeProjection {
    /// Verify projection evidence and exact generated-token coverage.
    #[napi(constructor)]
    pub fn new(
        projection_json: String,
        semantic_fingerprint_json: String,
        projection_fingerprint_json: String,
        registrations_json: String,
        schema_authority_json: Option<String>,
        projected_batch_materializer: Option<Function<'_, (), ()>>,
    ) -> napi::Result<Self> {
        Self::install(
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            registrations_json,
            schema_authority_json,
            projected_batch_materializer,
            true,
        )
    }

    fn install(
        projection_json: String,
        semantic_fingerprint_json: String,
        projection_fingerprint_json: String,
        registrations_json: String,
        schema_authority_json: Option<String>,
        projected_batch_materializer: Option<Function<'_, (), ()>>,
        require_projected_batch_materializer: bool,
    ) -> napi::Result<Self> {
        Self::install_with_policy(
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            registrations_json,
            schema_authority_json,
            projected_batch_materializer,
            if require_projected_batch_materializer {
                BatchMaterializerPolicy::RequiredSuccessor
            } else {
                BatchMaterializerPolicy::OptionalSuccessor
            },
        )
    }

    fn install_with_policy(
        projection_json: String,
        semantic_fingerprint_json: String,
        projection_fingerprint_json: String,
        registrations_json: String,
        schema_authority_json: Option<String>,
        projected_batch_materializer: Option<Function<'_, (), ()>>,
        policy: BatchMaterializerPolicy,
    ) -> napi::Result<Self> {
        let (runtime, managed_scope_id, schema_authority_evidence) = match schema_authority_json {
            Some(schema_authority_json) => {
                let (runtime, scope, declared) = install_authority_backed_projection(
                    &projection_json,
                    &semantic_fingerprint_json,
                    &projection_fingerprint_json,
                    &schema_authority_json,
                )?;
                (runtime, Some(scope), Some(declared))
            }
            None => {
                let runtime = decode_runtime_projection_verified(
                    projection_json.as_bytes(),
                    semantic_fingerprint_json.as_bytes(),
                    projection_fingerprint_json.as_bytes(),
                )
                .map_err(diagnostic_error)?;
                if runtime.target() != BindingTarget::TypeScript {
                    return Err(invalid_error(
                        "runtime projection does not target TypeScript",
                    ));
                }
                if projection_uses_ordered_collections(&runtime) {
                    return Err(projection_evidence_mismatch());
                }
                verify_legacy_typescript_projection_evidence(&runtime)?;
                (runtime, None, None)
            }
        };
        if projected_batch_materializer.is_none()
            && (matches!(policy, BatchMaterializerPolicy::PlanResultsOnly)
                || (projection_uses_ordered_collections(&runtime)
                    && matches!(policy, BatchMaterializerPolicy::RequiredSuccessor)))
        {
            return Err(invalid_error(
                "native projection requires exactly one generated batch materializer",
            ));
        }
        let projected_batch_materializer = projected_batch_materializer
            .map(|materializer| materializer.create_ref().map(Arc::new))
            .transpose()?;
        let registrations: Vec<ModelRegistration> = serde_json::from_str(&registrations_json)
            .map_err(|error| invalid_error(format!("invalid projection registrations: {error}")))?;
        if registrations.len() != runtime.models().len() {
            return Err(invalid_error(format!(
                "projection requires exactly {} model registrations, received {}",
                runtime.models().len(),
                registrations.len()
            )));
        }
        let mut covered = BTreeSet::new();
        for registration in registrations {
            let id = type_id_from_key(&registration.type_key)?;
            if !covered.insert(id.clone()) {
                return Err(invalid_error("duplicate projected model registration"));
            }
            let model = runtime.models().get(&id).ok_or_else(|| {
                invalid_error("registration references an unknown projected model")
            })?;
            if registration.target_name != model.target_name().as_str()
                || registration.create != model.create().enabled()
                || registration.reference != model.reference_read().target_name().is_some()
            {
                return Err(invalid_error(
                    "registration does not match the projected token facets",
                ));
            }
        }
        if covered.len() != runtime.models().len() {
            return Err(invalid_error(
                "projection model registration coverage is incomplete",
            ));
        }
        let mut types_by_label = BTreeMap::new();
        let mut projected_type_keys = BTreeMap::new();
        for id in runtime.models().keys() {
            if types_by_label
                .insert(id.label().as_str().to_owned(), id.clone())
                .is_some()
            {
                return Err(invalid_error(
                    "projection contains duplicate provider type labels",
                ));
            }
            let canonical = canonical_type_key(id)?;
            let json_string = serde_json::to_string(&canonical).map_err(json_error)?;
            let model = runtime
                .models()
                .get(id)
                .ok_or_else(|| runtime_error("projected role index model is absent"))?;
            let mut role_ids = Vec::new();
            role_ids
                .try_reserve_exact(model.complete_read().roles().len())
                .map_err(|_| runtime_error("failed to reserve projected role index"))?;
            let mut role_ordinals_by_target = BTreeMap::new();
            for (ordinal, role_id) in model.complete_read().roles().keys().enumerate() {
                let ordinal = u32::try_from(ordinal)
                    .map_err(|_| invalid_error("projection contains too many projected roles"))?;
                let target_name = model
                    .query_tokens()
                    .roles()
                    .get(role_id)
                    .ok_or_else(|| runtime_error("projected role has no generated token"))?
                    .target_name()
                    .as_str()
                    .to_owned();
                if role_ordinals_by_target
                    .insert(target_name, ordinal)
                    .is_some()
                {
                    return Err(invalid_error(
                        "projection contains duplicate generated role targets",
                    ));
                }
                role_ids.push(role_id.clone());
            }
            projected_type_keys.insert(
                id.clone(),
                ProjectedTypeKeyCache {
                    canonical,
                    json_string,
                    roles: ProjectedRoleIndex {
                        role_ids,
                        role_ordinals_by_target,
                    },
                },
            );
        }
        let mut projection = InstalledRuntimeProjection::try_new(runtime).map_err(orm_error)?;
        if let Some(authority) = &schema_authority_evidence {
            projection = projection.with_schema_authority(authority);
        }
        let projection = Arc::new(projection);
        Ok(Self {
            package: Arc::new(InstalledPackage {
                projection,
                managed_scope_id,
                types_by_label,
                projected_type_keys,
                projected_batch_materializer,
            }),
        })
    }

    /// Open a database through this installed package's verified authority.
    #[napi(js_name = "connectDirect")]
    #[allow(clippy::too_many_arguments)]
    pub fn connect_direct(
        &self,
        endpoint: String,
        database: String,
        username: String,
        password: String,
        http_port: u16,
        tls_mode: String,
        tls_root_ca: Option<String>,
        connection_limits: Option<&NodeQueryExecutionResources>,
        answer_limits: Option<&NodeQueryExecutionResources>,
        cancellation: Option<&NodeQueryCancellation>,
        query_limits: Option<&NodeQueryLimits>,
    ) -> napi::Result<NodeRustDatabase> {
        let tls = direct_tls_from_node(&tls_mode, tls_root_ca)?;
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
        if let Some(limits) = query_limits {
            policy = policy
                .query_limits(limits.inner())
                .map_err(|error| napi::Error::new(napi::Status::InvalidArg, error.to_string()))?;
        }
        let runtime = ProviderRuntimeOwner::new()
            .map(Arc::new)
            .map_err(|error| runtime_error(format!("Failed to create Tokio runtime: {error}")))?;
        let cancellation = cancellation
            .map(NodeQueryCancellation::inner)
            .unwrap_or_default();
        let database = runtime
            .block_on(Database::connect_direct(
                self.package.projection.as_ref(),
                &policy,
                cancellation,
            ))
            .map_err(napi_sdk_diagnostic)?;
        Ok(NodeRustDatabase::from_handles(
            Arc::new(database),
            runtime,
            self.package.managed_scope_id.clone(),
        ))
    }

    /// Bind one exact projected model to a database-owned manager.
    #[napi(js_name = "managerForDatabase")]
    pub fn manager_for_database(
        &self,
        type_key: String,
        database: &NodeRustDatabase,
    ) -> napi::Result<NodeProjectedModelManager> {
        let type_id = manageable_type(self.package.as_ref(), &type_key)?;
        let (database, runtime) = database.handles();
        Ok(NodeProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            target: Some(OwnedQueryTarget::Database(database)),
            successor_batch_marker: None,
            runtime,
            filters: vec![],
            closed_filter: None,
            has_empty_iids: false,
        })
    }

    /// Bind one exact projected model to a verified proxy connection.
    #[napi(js_name = "managerForProxy")]
    pub fn manager_for_proxy(
        &self,
        type_key: String,
        connection: &crate::proxy_connection_runtime::NodeProxyConnection,
    ) -> napi::Result<NodeProjectedModelManager> {
        let type_id = manageable_type(self.package.as_ref(), &type_key)?;
        let (connection, runtime) = connection.handles();
        Ok(NodeProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            target: Some(OwnedQueryTarget::Proxy(connection)),
            successor_batch_marker: None,
            runtime,
            filters: vec![],
            closed_filter: None,
            has_empty_iids: false,
        })
    }

    /// Bind one exact projected model to a borrowed transaction manager.
    #[napi(js_name = "managerForTransaction")]
    pub fn manager_for_transaction(
        &self,
        type_key: String,
        transaction: &NodeRustTransactionContext,
    ) -> napi::Result<NodeProjectedModelManager> {
        let type_id = manageable_type(self.package.as_ref(), &type_key)?;
        let successor_batch_marker = transaction.successor_batch_marker();
        let (transaction, runtime) = transaction.handles();
        Ok(NodeProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            target: Some(OwnedQueryTarget::Transaction(transaction)),
            successor_batch_marker: Some(successor_batch_marker),
            runtime,
            filters: vec![],
            closed_filter: None,
            has_empty_iids: false,
        })
    }

    /// Build an opaque match session carrying the common resource policy and
    /// caller-owned cooperative cancellation signal.
    ///
    /// `query_limits` replaces the canonical query limits for every query of
    /// the session; left out, the session keeps the defaults.
    #[napi(js_name = "matchSessionWithResources")]
    pub fn match_session_with_resources(
        &self,
        resources: &NodeQueryExecutionResources,
        cancellation: &NodeQueryCancellation,
        query_limits: Option<&NodeQueryLimits>,
    ) -> napi::Result<NodeMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        Ok(NodeMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            resources.inner(),
            cancellation.inner(),
            query_limits.map_or(
                type_bridge_contract::limits::QueryLimits::CANONICAL,
                NodeQueryLimits::inner,
            ),
        ))
    }

    /// Resolve one exact projected entity or relation token to its provider label.
    #[napi(js_name = "matchModelType")]
    pub fn match_model_type(&self, type_key: String) -> napi::Result<String> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        Ok(id.label().as_str().to_owned())
    }

    /// Validate one generated attribute scalar through the installed Rust projection.
    #[napi(js_name = "validateAttributeValueJson")]
    pub fn validate_attribute_value_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = type_id_from_key(&type_key)?;
        if id.kind() != TypeKind::Attribute {
            return Err(invalid_error(
                "generated scalar validation requires an attribute token",
            ));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection attribute model is absent"))?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| invalid_error("projection attribute has no scalar domain"))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|error| invalid_error(format!("invalid projected scalar wire: {error}")))?;
        let expected = ValueType::from(value_type);
        if projection_uses_ordered_collections(self.package.projection.projection()) {
            project_attribute_scalar(&self.package.projection, id, &wire, expected)
                .map(|_| ())
                .map_err(napi_sdk_diagnostic)
        } else {
            let value = scalar_to_attribute(&wire, expected)?;
            self.package
                .projection
                .validate_attribute_value(&id, &value)
                .map_err(|error| invalid_error(error.to_string()))
        }
    }

    /// Validate one generated provider-hydrated attribute scalar through the common Rust contract.
    #[napi(js_name = "validateHydratedAttributeValueJson")]
    pub fn validate_hydrated_attribute_value_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = type_id_from_key(&type_key)?;
        let path = [SdkDiagnosticPathSegment::Type(id.clone())];
        if id.kind() != TypeKind::Attribute {
            return Err(napi_sdk_diagnostic(malformed_projected_hydration_at(path)));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| {
                napi_sdk_diagnostic(generated_token_package_mismatch_at(path.clone()))
            })?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| napi_sdk_diagnostic(malformed_projected_hydration_at(path.clone())))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|_| napi_sdk_diagnostic(malformed_projected_hydration_at(path.clone())))?;
        project_hydrated_attribute_scalar(
            &self.package.projection,
            id,
            &wire,
            ValueType::from(value_type),
        )
        .map(|_| ())
        .map_err(napi_sdk_diagnostic)
    }

    /// Validate one generated owned-field scalar through the installed Rust projection.
    #[napi(js_name = "validateFieldValueJson")]
    pub fn validate_field_value_json(
        &self,
        type_key: String,
        field_name: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection model is absent"))?;
        let field = model
            .query_tokens()
            .fields()
            .values()
            .find(|field| field.target_name().as_str() == field_name)
            .ok_or_else(|| {
                invalid_error("generated value references an unknown projected field")
            })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                .map_err(diagnostic_error)?;
        let attribute = self
            .package
            .projection
            .projection()
            .models()
            .get(&attribute_id)
            .ok_or_else(|| invalid_error("projection field attribute is absent"))?;
        let value_type = attribute
            .declaration()
            .value_type()
            .ok_or_else(|| invalid_error("projection field attribute has no scalar domain"))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|error| invalid_error(format!("invalid projected scalar wire: {error}")))?;
        let expected = ValueType::from(value_type);
        if projection_uses_ordered_collections(self.package.projection.projection()) {
            let projected =
                project_attribute_scalar(&self.package.projection, attribute_id, &wire, expected)
                    .map_err(napi_sdk_diagnostic)?;
            self.package
                .projection
                .validate_canonical_field_value(&id, field.id(), projected.value())
                .map_err(napi_sdk_diagnostic)
        } else {
            let value = scalar_to_attribute(&wire, expected)?;
            self.package
                .projection
                .validate_field_value(&id, &field_name, &value)
                .map_err(|error| invalid_error(error.to_string()))
        }
    }

    /// Check how many values a generated model's create member holds against
    /// its projected cardinality. A refusal is the core's structured
    /// diagnostic.
    #[napi(js_name = "validateMemberCardinality")]
    pub fn validate_member_cardinality(
        &self,
        type_key: String,
        member_name: String,
        count: u32,
    ) -> napi::Result<()> {
        let id = type_id_from_key(&type_key)?;
        validate_projected_create_member_count(
            &self.package.projection,
            &id,
            &member_name,
            count as usize,
        )
        .map_err(napi_sdk_diagnostic)
    }

    /// Encode one exact generated attribute wire through the shared canonical record contract.
    #[napi(js_name = "encodeAttributeJson")]
    pub fn encode_attribute_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<Buffer> {
        let id = type_id_from_key(&type_key)?;
        if id.kind() != TypeKind::Attribute {
            return Err(invalid_error(
                "canonical attribute encoding requires an exact generated attribute type",
            ));
        }
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key {
            return Err(invalid_error(
                "attribute wire has the wrong exact generated type",
            ));
        }
        let projected =
            project_attribute_wire(self.package.as_ref(), &wire).map_err(napi_sdk_diagnostic)?;
        let record = type_bridge_orm::record_from_attribute(&self.package.projection, &projected)
            .map_err(|error| invalid_error(error.to_string()))?;
        record.encode().map(Buffer::from).map_err(diagnostic_error)
    }

    /// Decode canonical attribute bytes through exact installed package authority.
    #[napi(js_name = "decodeAttributeJson")]
    pub fn decode_attribute_json(&self, type_key: String, bytes: Buffer) -> napi::Result<String> {
        let expected = type_id_from_key(&type_key)?;
        if expected.kind() != TypeKind::Attribute {
            return Err(invalid_error(
                "canonical attribute decoding requires an exact generated attribute type",
            ));
        }
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
            .map_err(diagnostic_error)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| invalid_error(error.to_string()))?;
        let ProjectedCodecValue::Attribute(value) = value else {
            return Err(invalid_error("canonical record is not an attribute value"));
        };
        if value.attribute_type() != &expected {
            return Err(invalid_error(
                "canonical attribute has the wrong exact generated type",
            ));
        }
        wire_json(&projected_attribute_value_wire(
            self.package.as_ref(),
            &value,
        )?)
    }

    /// Validate one complete generated create payload through the common Rust contract.
    #[napi(js_name = "validateCreateJson")]
    pub fn validate_create_json(&self, type_key: String, value_json: String) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key || wire.form != WireForm::Complete {
            return Err(napi_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        if wire.iid.is_some() || wire.value.is_some() {
            return Err(napi_sdk_diagnostic(malformed_projected_create_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_create_wire(self.package.as_ref(), &id, &wire)
            .map(|_| ())
            .map_err(napi_sdk_diagnostic)
    }

    /// Encode one exact generated create wire through the shared canonical record contract.
    #[napi(js_name = "encodeCreateJson")]
    pub fn encode_create_json(&self, type_key: String, value_json: String) -> napi::Result<Buffer> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key || wire.form != WireForm::Complete {
            return Err(invalid_error(
                "create wire has the wrong exact generated type",
            ));
        }
        let projected =
            project_create_wire(self.package.as_ref(), &id, &wire).map_err(napi_sdk_diagnostic)?;
        let record = type_bridge_orm::record_from_create(&self.package.projection, &projected)
            .map_err(|error| invalid_error(error.to_string()))?;
        record.encode().map(Buffer::from).map_err(diagnostic_error)
    }

    /// Decode canonical create bytes through exact installed package authority.
    #[napi(js_name = "decodeCreateJson")]
    pub fn decode_create_json(&self, type_key: String, bytes: Buffer) -> napi::Result<String> {
        let expected = manageable_type(self.package.as_ref(), &type_key)?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
            .map_err(diagnostic_error)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| invalid_error(error.to_string()))?;
        let ProjectedCodecValue::Create(value) = value else {
            return Err(invalid_error("canonical record is not a create payload"));
        };
        if value.type_id() != &expected {
            return Err(invalid_error(
                "canonical create has the wrong exact generated type",
            ));
        }
        wire_json(&projected_create_wire(self.package.as_ref(), &value)?)
    }

    /// Encode one exact generated detached reference as canonical bytes.
    #[napi(js_name = "encodeReferenceJson")]
    pub fn encode_reference_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<Buffer> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key || wire.form != WireForm::Reference {
            return Err(invalid_error(
                "reference wire has the wrong exact generated type",
            ));
        }
        let allowed = BTreeSet::from([ProjectedModelUse::new(id, ProjectedModelForm::Reference)]);
        let projected = project_reference_wire(self.package.as_ref(), &allowed, &wire, &[])
            .map_err(napi_sdk_diagnostic)?;
        let record = type_bridge_orm::record_from_reference(&self.package.projection, &projected)
            .map_err(|error| invalid_error(error.to_string()))?;
        record.encode().map(Buffer::from).map_err(diagnostic_error)
    }

    /// Decode canonical reference bytes through exact installed package authority.
    #[napi(js_name = "decodeReferenceJson")]
    pub fn decode_reference_json(&self, type_key: String, bytes: Buffer) -> napi::Result<String> {
        let expected = manageable_type(self.package.as_ref(), &type_key)?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
            .map_err(diagnostic_error)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| invalid_error(error.to_string()))?;
        let ProjectedCodecValue::Reference(value) = value else {
            return Err(invalid_error("canonical record is not a reference"));
        };
        if value.type_id() != &expected {
            return Err(invalid_error(
                "canonical reference has the wrong exact generated type",
            ));
        }
        wire_json(&projected_detached_reference_wire(
            self.package.as_ref(),
            &value,
        )?)
    }

    /// Encode one exact generated hydrated model as a detached canonical snapshot.
    #[napi(js_name = "encodeSnapshotJson")]
    pub fn encode_snapshot_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<Buffer> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key || wire.form != WireForm::Complete || wire.iid.is_none() {
            return Err(invalid_error(
                "snapshot wire has the wrong exact generated type or no IID",
            ));
        }
        let projected =
            project_thing_wire(self.package.as_ref(), &id, &wire).map_err(napi_sdk_diagnostic)?;
        let record = type_bridge_orm::record_from_snapshot(&self.package.projection, &projected)
            .map_err(|error| invalid_error(error.to_string()))?;
        record.encode().map(Buffer::from).map_err(diagnostic_error)
    }

    /// Decode canonical snapshot bytes through exact installed package authority.
    #[napi(js_name = "decodeSnapshotJson")]
    pub fn decode_snapshot_json(&self, type_key: String, bytes: Buffer) -> napi::Result<String> {
        let expected = manageable_type(self.package.as_ref(), &type_key)?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
            .map_err(diagnostic_error)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| invalid_error(error.to_string()))?;
        let ProjectedCodecValue::Snapshot(value) = value else {
            return Err(invalid_error("canonical record is not a snapshot"));
        };
        if value.type_id() != &expected {
            return Err(invalid_error(
                "canonical snapshot has the wrong exact generated type",
            ));
        }
        wire_json(&projected_thing_wire(self.package.as_ref(), &value)?)
    }

    /// Create the opaque mutation fence installed on one decoded snapshot facade.
    #[napi(js_name = "detachedSnapshotProof")]
    pub fn detached_snapshot_proof(&self) -> External<NodeProjectedFacadeProof> {
        External::new(NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::DetachedSnapshot,
        })
    }

    /// Encode one exact generated struct value as canonical bytes.
    #[napi(js_name = "encodeStructJson")]
    pub fn encode_struct_json(&self, type_key: String, value_json: String) -> napi::Result<Buffer> {
        let id = type_id_from_key(&type_key)?;
        if id.kind() != TypeKind::Struct {
            return Err(invalid_error("struct wire has a non-struct type"));
        }
        let wire: ProjectedStructWire = serde_json::from_str(&value_json).map_err(json_error)?;
        if wire.type_key != type_key {
            return Err(invalid_error(
                "struct wire has the wrong exact generated type",
            ));
        }
        let projection = self
            .package
            .projection
            .projection()
            .structs()
            .values()
            .find(|projection| projection.id().label() == id.label())
            .ok_or_else(|| invalid_error("struct is absent from installed projection"))?;
        if wire.values.len() != projection.fields().len() {
            return Err(invalid_error(
                "struct wire has the wrong exact member inventory",
            ));
        }
        let members = projection
            .fields()
            .iter()
            .map(|field| {
                let value = wire
                    .values
                    .get(field.target_name().as_str())
                    .ok_or_else(|| invalid_error("struct wire omitted a declared member"))?;
                value
                    .as_ref()
                    .map(|wire| {
                        scalar_to_ordered_attribute(wire, ValueType::from(field.value_type()))
                            .and_then(|value| canonical_struct_scalar(&value))
                    })
                    .transpose()
            })
            .collect::<napi::Result<Vec<_>>>()?;
        let projected = ProjectedStructValue::try_new(&self.package.projection, id, members)
            .map_err(|error| invalid_error(error.to_string()))?;
        let record = type_bridge_orm::record_from_struct(&self.package.projection, &projected)
            .map_err(|error| invalid_error(error.to_string()))?;
        record.encode().map(Buffer::from).map_err(diagnostic_error)
    }

    /// Decode canonical struct bytes through exact installed package authority.
    #[napi(js_name = "decodeStructJson")]
    pub fn decode_struct_json(&self, type_key: String, bytes: Buffer) -> napi::Result<String> {
        let expected = type_id_from_key(&type_key)?;
        if expected.kind() != TypeKind::Struct {
            return Err(invalid_error("requested canonical type is not a struct"));
        }
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
            .map_err(diagnostic_error)?;
        let value = type_bridge_orm::materialize_record(&self.package.projection, &record)
            .map_err(|error| invalid_error(error.to_string()))?;
        let ProjectedCodecValue::Struct(value) = value else {
            return Err(invalid_error("canonical record is not a struct"));
        };
        if value.type_id() != &expected {
            return Err(invalid_error(
                "canonical struct has the wrong exact generated type",
            ));
        }
        serde_json::to_string(&projected_struct_wire(self.package.as_ref(), &value))
            .map_err(json_error)
    }

    /// Encode one nominal record with cancellation, deadline, and resource limits.
    #[napi(js_name = "encodeRecordJsonControlled")]
    #[allow(clippy::too_many_arguments)]
    pub fn encode_record_json_controlled(
        &self,
        record_kind: String,
        type_key: String,
        value_json: String,
        cancellation: Option<&NodeQueryCancellation>,
        timeout_milliseconds: Option<u32>,
        max_input_bytes: Option<u32>,
        max_output_bytes: Option<u32>,
        max_depth: Option<u32>,
        max_members: Option<u32>,
    ) -> napi::Result<Buffer> {
        let record_kind = parse_enum::<ProjectedRecordKind>(&record_kind)?;
        let control = NodeCanonicalControl::capture(
            cancellation,
            timeout_milliseconds,
            max_input_bytes,
            max_output_bytes,
            max_depth,
            None,
            max_members,
        )?;
        control.check()?;
        if value_json.len() > control.max_input_bytes {
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_input_limit(),
            ));
        }
        let bytes = match record_kind {
            ProjectedRecordKind::Attribute => self.encode_attribute_json(type_key, value_json),
            ProjectedRecordKind::Create => self.encode_create_json(type_key, value_json),
            ProjectedRecordKind::Reference => self.encode_reference_json(type_key, value_json),
            ProjectedRecordKind::Snapshot => self.encode_snapshot_json(type_key, value_json),
            ProjectedRecordKind::Struct => self.encode_struct_json(type_key, value_json),
        }?;
        control.check()?;
        let record = type_bridge_contract::projected_record::ProjectedRecord::decode_with_limits(
            &bytes,
            control.output_limits(),
        )
        .map_err(|error| node_canonical_limit_error(error, false))?;
        if record.decoded_weight() > control.max_members {
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let bytes = record
            .encode_with_limits(control.output_limits())
            .map_err(|error| node_canonical_limit_error(error, false))?;
        control.check()?;
        Ok(Buffer::from(bytes))
    }

    /// Decode one nominal record with cancellation, deadline, and resource limits.
    #[napi(js_name = "decodeRecordJsonControlled")]
    #[allow(clippy::too_many_arguments)]
    pub fn decode_record_json_controlled(
        &self,
        record_kind: String,
        type_key: String,
        bytes: Buffer,
        cancellation: Option<&NodeQueryCancellation>,
        timeout_milliseconds: Option<u32>,
        max_input_bytes: Option<u32>,
        max_output_bytes: Option<u32>,
        max_depth: Option<u32>,
        max_members: Option<u32>,
    ) -> napi::Result<String> {
        let record_kind = parse_enum::<ProjectedRecordKind>(&record_kind)?;
        let control = NodeCanonicalControl::capture(
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
            &bytes,
            control.input_limits(),
        )
        .map_err(|error| node_canonical_limit_error(error, true))?;
        if record.decoded_weight() > control.max_members {
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let canonical = record
            .encode_with_limits(control.output_limits())
            .map_err(|error| node_canonical_limit_error(error, false))?;
        control.check()?;
        let value_json = match record_kind {
            ProjectedRecordKind::Attribute => {
                self.decode_attribute_json(type_key, Buffer::from(canonical))
            }
            ProjectedRecordKind::Create => {
                self.decode_create_json(type_key, Buffer::from(canonical))
            }
            ProjectedRecordKind::Reference => {
                self.decode_reference_json(type_key, Buffer::from(canonical))
            }
            ProjectedRecordKind::Snapshot => {
                self.decode_snapshot_json(type_key, Buffer::from(canonical))
            }
            ProjectedRecordKind::Struct => {
                self.decode_struct_json(type_key, Buffer::from(canonical))
            }
        }?;
        if value_json.len() > control.max_output_bytes {
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_output_limit(),
            ));
        }
        control.check()?;
        Ok(value_json)
    }

    /// Compose already canonical exact-package records into one deterministic archive.
    #[napi(js_name = "encodeArchive")]
    pub fn encode_archive(&self, records: Vec<Buffer>) -> napi::Result<Buffer> {
        let mut verified = Vec::with_capacity(records.len());
        for bytes in records {
            let record = type_bridge_contract::projected_record::ProjectedRecord::decode(&bytes)
                .map_err(diagnostic_error)?;
            let _ = type_bridge_orm::materialize_record(&self.package.projection, &record)
                .map_err(node_canonical_codec_error)?;
            verified.push(record);
        }
        type_bridge_contract::projected_record::ProjectedArchive::try_new(verified)
            .and_then(|archive| archive.encode())
            .map(Buffer::from)
            .map_err(diagnostic_error)
    }

    /// Compose records with cancellation, deadline, and tighten-only limits.
    #[napi(js_name = "encodeArchiveControlled")]
    #[allow(clippy::too_many_arguments)]
    pub fn encode_archive_controlled(
        &self,
        records: Vec<Buffer>,
        cancellation: Option<&NodeQueryCancellation>,
        timeout_milliseconds: Option<u32>,
        max_input_bytes: Option<u32>,
        max_output_bytes: Option<u32>,
        max_depth: Option<u32>,
        max_records: Option<u32>,
        max_members: Option<u32>,
    ) -> napi::Result<Buffer> {
        let control = NodeCanonicalControl::capture(
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
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        let mut verified = Vec::with_capacity(records.len());
        for bytes in records {
            control.check()?;
            let record =
                type_bridge_contract::projected_record::ProjectedRecord::decode_with_limits(
                    &bytes,
                    control.input_limits(),
                )
                .map_err(|error| node_canonical_limit_error(error, true))?;
            let _ = type_bridge_orm::materialize_record(&self.package.projection, &record)
                .map_err(node_canonical_codec_error)?;
            verified.push(record);
        }
        let archive = type_bridge_contract::projected_record::ProjectedArchive::try_new(verified)
            .map_err(diagnostic_error)?;
        if archive.decoded_weight() > control.max_members {
            return Err(napi_sdk_diagnostic(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ));
        }
        control.check()?;
        let bytes = archive
            .encode_with_limits(control.output_limits())
            .map_err(|error| node_canonical_limit_error(error, false))?;
        control.check()?;
        Ok(Buffer::from(bytes))
    }

    /// Decode one complete archive into canonical individual exact-package records.
    #[napi(js_name = "decodeArchive")]
    pub fn decode_archive(&self, bytes: Buffer) -> napi::Result<Vec<Buffer>> {
        let archive = type_bridge_contract::projected_record::ProjectedArchive::decode(&bytes)
            .map_err(diagnostic_error)?;
        archive
            .records()
            .iter()
            .map(|record| {
                let _ = type_bridge_orm::materialize_record(&self.package.projection, record)
                    .map_err(node_canonical_codec_error)?;
                record.encode().map(Buffer::from).map_err(diagnostic_error)
            })
            .collect()
    }

    /// Decode an archive with cancellation, deadline, and tighten-only limits.
    #[napi(js_name = "decodeArchiveControlled")]
    #[allow(clippy::too_many_arguments)]
    pub fn decode_archive_controlled(
        &self,
        bytes: Buffer,
        cancellation: Option<&NodeQueryCancellation>,
        timeout_milliseconds: Option<u32>,
        max_input_bytes: Option<u32>,
        max_output_bytes: Option<u32>,
        max_depth: Option<u32>,
        max_records: Option<u32>,
        max_members: Option<u32>,
    ) -> napi::Result<Vec<Buffer>> {
        let control = NodeCanonicalControl::capture(
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
            &bytes,
            control.input_limits(),
        )
        .map_err(|error| node_canonical_limit_error(error, true))?;
        if archive.records().len() > control.max_records
            || archive.decoded_weight() > control.max_members
        {
            return Err(napi_sdk_diagnostic(
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
                    .map_err(node_canonical_codec_error)?;
                let bytes = record
                    .encode_with_limits(control.output_limits())
                    .map_err(|error| node_canonical_limit_error(error, false))?;
                output_bytes = output_bytes.saturating_add(bytes.len());
                if output_bytes > control.max_output_bytes {
                    return Err(napi_sdk_diagnostic(
                        SdkExecutionDiagnostic::projected_codec_output_limit(),
                    ));
                }
                Ok(Buffer::from(bytes))
            })
            .collect::<napi::Result<Vec<_>>>()?;
        control.check()?;
        Ok(records)
    }

    /// Validate one complete generated provider result through the common Rust contract.
    #[napi(js_name = "validateThingJson")]
    pub fn validate_thing_json(&self, type_key: String, value_json: String) -> napi::Result<()> {
        self.validate_thing_json_inner(&type_key, &value_json)
    }

    fn validate_thing_json_inner(&self, type_key: &str, value_json: &str) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), type_key)?;
        let wire: ProjectedWire = serde_json::from_str(value_json).map_err(|_| {
            napi_sdk_diagnostic(malformed_projected_hydration_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ]))
        })?;
        if wire.type_key != type_key || wire.form != WireForm::Complete {
            return Err(napi_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_thing_wire(self.package.as_ref(), &id, &wire)
            .map(|_| ())
            .map_err(napi_sdk_diagnostic)
    }

    /// Surface a stable package-boundary diagnostic at an exact create member path.
    #[napi(js_name = "rejectGeneratedTokenPackageMismatch")]
    pub fn reject_generated_token_package_mismatch(&self, path_json: String) -> napi::Result<()> {
        let path: Vec<GeneratedPackagePathWire> = serde_json::from_str(&path_json)
            .map_err(|error| invalid_error(format!("invalid generated package path: {error}")))?;
        let path = generated_package_path(self.package.as_ref(), &path)?;
        Err(napi_sdk_diagnostic(generated_token_package_mismatch_at(
            path,
        )))
    }

    /// Revalidate a diagnostic against this exact installed projection.
    #[napi(js_name = "revalidateMatchDiagnostic")]
    pub fn revalidate_match_diagnostic(&self, diagnostic: String) -> napi::Result<String> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        revalidate_diagnostic(&registry, &diagnostic)
    }

    /// Materialize one validated match thing as the package's private wire.
    #[napi(js_name = "materializeMatchThingJson")]
    pub fn materialize_match_thing_json(
        &self,
        thing: &NodeValidatedThingHandle,
    ) -> napi::Result<String> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        let concrete = registry
            .descriptor_type_name(thing.hydrated_descriptor())
            .ok_or_else(|| runtime_error("validated result descriptor is no longer registered"))?;
        let kind = match thing.hydrated_kind() {
            ThingKind::Entity => TypeKind::Entity,
            ThingKind::Relation => TypeKind::Relation,
        };
        let id = self.package.type_by_label(&concrete)?.clone();
        if id.kind() != kind {
            return Err(runtime_error(
                "validated match thing kind disagrees with the installed projection",
            ));
        }
        let attributes = match_attributes(self.package.as_ref(), &id, thing.hydrated_attributes())?;
        let wire = match kind {
            TypeKind::Entity => hydrate_entity(
                self.package.as_ref(),
                &id,
                &DynamicEntityRow {
                    iid: Some(thing.hydrated_concept_id().to_owned()),
                    type_name: Some(concrete),
                    attributes,
                },
            )?,
            TypeKind::Relation => {
                let mut role_players = Vec::new();
                for role in thing.hydrated_roles() {
                    for player in role.players() {
                        let player_type = registry
                            .descriptor_type_name(player.concrete_descriptor())
                            .ok_or_else(|| {
                                runtime_error(
                                    "validated role-player descriptor is no longer registered",
                                )
                            })?;
                        let player_id = self.package.type_by_label(&player_type)?.clone();
                        let player_kind = match player.kind() {
                            ThingKind::Entity => TypeKind::Entity,
                            ThingKind::Relation => TypeKind::Relation,
                        };
                        if player_id.kind() != player_kind {
                            return Err(runtime_error(
                                "validated role player kind disagrees with the projection",
                            ));
                        }
                        let player_attributes = match_attributes(
                            self.package.as_ref(),
                            &player_id,
                            player.attributes(),
                        )?
                        .into_iter()
                        .map(|(name, value)| (name, attribute_json_value(&value)))
                        .collect();
                        role_players.push(DynamicRolePlayer {
                            role_name: role.role().name.clone(),
                            player_iid: Some(player.concept_id().as_str().to_owned()),
                            player_type_name: Some(player_type),
                            attributes: player_attributes,
                        });
                    }
                }
                hydrate_relation(
                    self.package.as_ref(),
                    &id,
                    &DynamicRelationRow {
                        iid: Some(thing.hydrated_concept_id().to_owned()),
                        type_name: Some(concrete),
                        attributes,
                        role_players,
                    },
                )?
            }
            TypeKind::Attribute | TypeKind::Struct => {
                unreachable!("match things are always entities or relations")
            }
        };
        serde_json::to_string(&wire).map_err(json_error)
    }

    /// Materialize one successor query thing with its exact opaque proof.
    #[napi(js_name = "materializeMatchThingProjected")]
    pub fn materialize_match_thing_projected(
        &self,
        thing: &NodeValidatedThingHandle,
    ) -> napi::Result<NodeProjectedValueEnvelope> {
        let projected = thing.projected_thing().ok_or_else(|| {
            runtime_error("validated query thing has no projected successor companion")
        })?;
        projected_envelope_arc(Arc::clone(&self.package), projected)
    }
}

/// Exact CRUD manager backed only by verified projection descriptors.
#[napi]
pub struct NodeProjectedModelManager {
    package: Arc<InstalledPackage>,
    type_id: TypeId,
    target: Option<OwnedQueryTarget>,
    successor_batch_marker: Option<Arc<AtomicBool>>,
    runtime: Arc<ProviderRuntimeOwner>,
    filters: Vec<DynamicExpr>,
    closed_filter: Option<ProjectedManagerFilter>,
    has_empty_iids: bool,
}

/// One immutable exact-model field-token filter for generated successor managers.
#[napi]
pub struct NodeProjectedManagerFilter {
    package: Arc<InstalledPackage>,
    filter: ProjectedManagerFilter,
    target: Option<OwnedQueryTarget>,
    runtime: Arc<ProviderRuntimeOwner>,
    successor_batch_marker: Option<Arc<AtomicBool>>,
}

#[napi]
impl NodeProjectedManagerFilter {
    /// Append one exact generated field comparison and return an immutable sibling.
    #[napi(js_name = "andProjected")]
    pub fn and_projected(
        &self,
        field_owner_type_key: String,
        field_attribute_key: String,
        comparison: String,
        value_json: String,
    ) -> napi::Result<Self> {
        let owner = type_id_from_key(&field_owner_type_key)?;
        let attribute_label: String = serde_json::from_str(&field_attribute_key)
            .map_err(|error| invalid_error(format!("invalid canonical attribute key: {error}")))?;
        let attribute = AttributeId::new(attribute_label).map_err(diagnostic_error)?;
        let field = OwnsFactId::new(owner.clone(), attribute).map_err(diagnostic_error)?;
        let identity = ProjectedTokenIdentity::Field { owner, field };
        let comparison = parse_enum::<ProjectedManagerComparison>(&comparison)?;
        let wire = parse_wire(&value_json)?;
        let value =
            project_attribute_wire(self.package.as_ref(), &wire).map_err(napi_sdk_diagnostic)?;
        let filter = self
            .filter
            .try_and(
                self.package.projection.as_ref(),
                &identity,
                comparison,
                &value,
            )
            .map_err(napi_sdk_diagnostic)?;
        Ok(Self {
            package: Arc::clone(&self.package),
            filter,
            target: self.target.clone(),
            runtime: Arc::clone(&self.runtime),
            successor_batch_marker: self.successor_batch_marker.clone(),
        })
    }

    /// Reject a field token that was not issued by this generated package.
    #[napi(js_name = "rejectForeignFieldToken")]
    pub fn reject_foreign_field_token(&self) -> napi::Result<()> {
        Err(napi_sdk_diagnostic(generated_token_package_mismatch_at([
            SdkDiagnosticPathSegment::Type(self.filter.model().clone()),
            SdkDiagnosticPathSegment::Argument(
                SdkDiagnosticName::new("predicates")
                    .expect("static manager predicate argument is canonical"),
            ),
        ])))
    }

    /// Delete every match through the shared canonical mutation executor.
    #[napi]
    pub fn delete(&self) -> napi::Result<()> {
        let executor = ProjectedManagerFilterExecutor::new(&self.package.projection);
        self.runtime.block_on(async {
            let target = self.target()?;
            let transaction = self.target.as_ref().and_then(OwnedQueryTarget::transaction);
            let prior = match transaction {
                Some(transaction) => Some(transaction.lifecycle_state().await),
                None => None,
            };
            let result = executor
                .delete_on_target(target, &self.filter, self.control())
                .await;
            if let (Some(transaction), Some(marker)) = (transaction, &self.successor_batch_marker)
                && !self.filter.matches_none()
                && matches!(
                    prior,
                    Some(TransactionContextState::Active | TransactionContextState::RolledBack)
                )
                && (result.is_ok()
                    || transaction.lifecycle_state().await == TransactionContextState::RollbackOnly)
            {
                marker.store(true, Ordering::Release);
            }
            result.map_err(|error| {
                napi_sdk_diagnostic(error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                ))
            })
        })
    }

    /// Return all exact-model matches with opaque successor proof retention.
    #[napi(js_name = "allProjected")]
    pub fn all_projected(&self) -> napi::Result<Vec<NodeProjectedValueEnvelope>> {
        self.execute_all()?
            .into_iter()
            .map(|thing| projected_envelope_arc(Arc::clone(&self.package), thing))
            .collect()
    }

    /// Return the optional identity-proven exact-model match.
    #[napi(js_name = "firstProjected")]
    pub fn first_projected(&self) -> napi::Result<Option<NodeProjectedValueEnvelope>> {
        self.execute_first()?
            .map(|thing| projected_envelope_arc(Arc::clone(&self.package), thing))
            .transpose()
    }

    /// Count exact-model matches without materializing them.
    #[napi]
    pub fn count(&self) -> napi::Result<BigInt> {
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.count_on_target(self.target()?, &self.filter, self.control()))
            .map(BigInt::from)
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    /// Test whether an exact-model match exists without materializing it.
    #[napi]
    pub fn exists(&self) -> napi::Result<bool> {
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.exists_on_target(self.target()?, &self.filter, self.control()))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }
}

impl NodeProjectedManagerFilter {
    fn target(&self) -> napi::Result<QueryTarget<'_>> {
        self.target
            .as_ref()
            .map(OwnedQueryTarget::as_target)
            .ok_or_else(|| runtime_error("projected manager filter has no execution target"))
    }

    fn control(&self) -> ProjectedManagerFilterInvocationControl {
        ProjectedManagerFilterInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    fn execute_all(&self) -> napi::Result<Vec<Arc<ProjectedThing>>> {
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.all_on_target(self.target()?, &self.filter, self.control()))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    fn execute_first(&self) -> napi::Result<Option<Arc<ProjectedThing>>> {
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.first_on_target(self.target()?, &self.filter, self.control()))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }
}

#[napi]
impl NodeProjectedModelManager {
    /// Insert one projected value through the common projected executor.
    #[napi(js_name = "insertProjected")]
    pub fn insert_projected<'env>(
        &self,
        env: Env,
        instance_json: String,
        proofs: Unknown<'env>,
    ) -> napi::Result<Object<'env>> {
        let proofs =
            raw_projected_input_proofs(proofs.value().env, proofs.raw(), &self.package, None)
                .map_err(RawProjectedBindingError::into_napi)?;
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        self.write_single_materialized(&env, instance, ProjectedBatchOperation::Insert, None)
    }

    /// Put one projected value through the common projected executor.
    #[napi(js_name = "putProjected")]
    pub fn put_projected<'env>(
        &self,
        env: Env,
        instance_json: String,
        proofs: Unknown<'env>,
    ) -> napi::Result<Object<'env>> {
        let proofs =
            raw_projected_input_proofs(proofs.value().env, proofs.raw(), &self.package, None)
                .map_err(RawProjectedBindingError::into_napi)?;
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        self.write_single_materialized(&env, instance, ProjectedBatchOperation::Put, None)
    }

    /// Replace one projected value through the common projected executor.
    #[napi(js_name = "updateProjected")]
    pub fn update_projected<'env>(
        &self,
        env: Env,
        iid: String,
        instance_json: String,
        proofs: Unknown<'env>,
    ) -> napi::Result<Object<'env>> {
        let proofs =
            raw_projected_input_proofs(proofs.value().env, proofs.raw(), &self.package, None)
                .map_err(RawProjectedBindingError::into_napi)?;
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        self.write_single_materialized(&env, instance, ProjectedBatchOperation::Update, Some(iid))
    }

    /// Insert projected values atomically through the common executor.
    #[napi(js_name = "insertManyProjected")]
    pub fn insert_many_projected<'env>(
        &self,
        env: Env,
        row_count: u32,
        row_at: Function<'env, u32, Unknown<'env>>,
    ) -> napi::Result<Object<'env>> {
        self.write_many_projected(
            &env,
            row_count,
            row_at.raw(),
            ProjectedBatchOperation::Insert,
        )
    }

    /// Put projected values atomically through the common executor.
    #[napi(js_name = "putManyProjected")]
    pub fn put_many_projected<'env>(
        &self,
        env: Env,
        row_count: u32,
        row_at: Function<'env, u32, Unknown<'env>>,
    ) -> napi::Result<Object<'env>> {
        self.write_many_projected(&env, row_count, row_at.raw(), ProjectedBatchOperation::Put)
    }

    /// Replace projected values atomically through the common executor.
    #[napi(js_name = "updateManyProjected")]
    pub fn update_many_projected<'env>(
        &self,
        env: Env,
        row_count: u32,
        row_at: Function<'env, u32, Unknown<'env>>,
    ) -> napi::Result<Object<'env>> {
        self.write_many_projected(
            &env,
            row_count,
            row_at.raw(),
            ProjectedBatchOperation::Update,
        )
    }

    /// Delete projected values atomically by canonical TypeDB IID.
    #[napi(js_name = "deleteManyProjected")]
    pub fn delete_many_projected<'env>(
        &self,
        env: Env,
        row_count: u32,
        row_at: Function<'env, u32, Unknown<'env>>,
    ) -> napi::Result<()> {
        let (batch, control) = capture_projected_batch(
            env.raw(),
            self.package.as_ref(),
            &self.type_id,
            ProjectedBatchOperation::Delete,
            row_count,
            row_at.raw(),
        )?;
        self.execute_projected_batch(&batch, control, false, (), |result| match result {
            ProjectedBatchResult::Deleted => Ok(()),
            _ => Err(SdkExecutionDiagnostic::internal_failure()),
        })
        .map_err(napi_sdk_diagnostic)
    }

    /// Permanent redirect to the IID filter first terminal.
    #[napi(js_name = "getByIidProjected")]
    pub fn get_by_iid_projected(
        &self,
        iid: String,
    ) -> napi::Result<Option<NodeProjectedValueEnvelope>> {
        self.filter_iid(&Iid::new(iid).map_err(diagnostic_error)?)?
            .first_projected()
    }

    /// Delete one exact value and retain its stored pre-delete snapshot.
    #[napi(js_name = "deleteByIidReturning")]
    pub fn delete_by_iid_returning<'env>(
        &self,
        env: Env,
        iid: String,
    ) -> napi::Result<Object<'env>> {
        let (batch, control) = self.prepare_returning_delete(vec![iid])?;
        self.execute_materialized_batch(&env, &batch, control, true, true)
    }

    /// Delete exact values atomically and return existing snapshots in input order.
    #[napi(js_name = "deleteManyReturning")]
    pub fn delete_many_returning<'env>(
        &self,
        env: Env,
        iids: Vec<String>,
    ) -> napi::Result<Object<'env>> {
        let (batch, control) = self.prepare_returning_delete(iids)?;
        self.execute_materialized_batch(&env, &batch, control, true, false)
    }

    /// Permanent redirect to the IID filter delete terminal.
    #[napi(js_name = "deleteByIid")]
    pub fn delete_by_iid(&self, iid: String) -> napi::Result<()> {
        self.filter_iid(&Iid::new(iid).map_err(diagnostic_error)?)?
            .delete()
    }

    /// Start one immutable canonical field-token filter for a generated package.
    #[napi(js_name = "managerFilter")]
    pub fn manager_filter(&self) -> napi::Result<NodeProjectedManagerFilter> {
        let filter = if self.filters.is_empty() {
            ProjectedManagerFilter::try_new(self.package.projection.as_ref(), self.type_id.clone())
                .map_err(napi_sdk_diagnostic)?
        } else {
            self.closed_filter.clone().ok_or_else(|| {
                napi_sdk_diagnostic(SdkExecutionDiagnostic::invalid_input(
                    SdkDiagnosticCode::new("unsupported_manager_filter").expect("static code"),
                    SdkDiagnosticMessage::new("This keyword filter cannot enter the canonical filter path; read its matches then use explicit instance or IID batch operations").expect("static message"),
                ))
            })?
        };
        Ok(NodeProjectedManagerFilter {
            package: Arc::clone(&self.package),
            filter,
            target: self.target.clone(),
            runtime: Arc::clone(&self.runtime),
            successor_batch_marker: self.successor_batch_marker.clone(),
        })
    }

    /// Preserve authored object-entry order for successor compatibility filters.
    #[napi(js_name = "filterEntriesJson")]
    pub fn filter_entries_json(&self, filters_json: String) -> napi::Result<Self> {
        let filters: Vec<OrderedManagerFilterEntry> =
            serde_json::from_str(&filters_json).map_err(|error| {
                invalid_error(format!(
                    "invalid ordered projected manager filters: {error}"
                ))
            })?;
        let descriptor = self.descriptor()?;
        let attributes = match &descriptor {
            TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
            TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
        };
        let mut combined = self.filters.clone();
        let closed = if self.filters.is_empty() {
            Some(self.manager_filter()?.filter)
        } else {
            self.closed_filter.clone()
        };
        let lowered = lower_filter_values(self.package.as_ref(), attributes, filters, closed)?;
        combined.extend(lowered.expressions);
        Ok(Self {
            package: Arc::clone(&self.package),
            type_id: self.type_id.clone(),
            target: self.target.clone(),
            successor_batch_marker: self.successor_batch_marker.clone(),
            runtime: Arc::clone(&self.runtime),
            filters: combined,
            closed_filter: lowered.closed,
            has_empty_iids: self.has_empty_iids || lowered.empty_iids,
        })
    }

    /// Fetch all values whose concrete type exactly matches this projection.
    #[napi(js_name = "allJson")]
    pub fn all_json(&self) -> napi::Result<String> {
        serde_json::to_string(&self.read_all_values()?).map_err(json_error)
    }

    /// Fetch the first exact filtered value, or `null` when no value matches.
    #[napi(js_name = "firstJson")]
    pub fn first_json(&self) -> napi::Result<String> {
        let value = self
            .read_first_projected()?
            .map(|thing| projected_thing_wire(self.package.as_ref(), thing.as_ref()))
            .transpose()?;
        serde_json::to_string(&value).map_err(json_error)
    }

    /// Read keyword-filter matches with opaque native origin retention.
    #[napi(js_name = "allProjected")]
    pub fn all_projected(&self) -> napi::Result<Vec<NodeProjectedValueEnvelope>> {
        self.read_all_projected()?
            .into_iter()
            .map(|thing| projected_envelope_arc(Arc::clone(&self.package), thing))
            .collect()
    }

    /// Read the arbitrary first keyword-filter match with native origin retention.
    #[napi(js_name = "firstProjected")]
    pub fn first_projected(&self) -> napi::Result<Option<NodeProjectedValueEnvelope>> {
        self.read_first_projected()?
            .map(|thing| projected_envelope_arc(Arc::clone(&self.package), thing))
            .transpose()
    }

    /// Count exact filtered values.
    #[napi]
    pub fn count(&self) -> napi::Result<BigInt> {
        if let Some(filter) = self.canonical_read_filter()? {
            return self.filter_handle(filter).count();
        }
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.compatibility_count_on_target(
                self.execution_target()?,
                &self.type_id,
                &self.filters,
                self.read_control(),
            ))
            .map(BigInt::from)
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    /// Return whether at least one exact filtered value exists.
    #[napi]
    pub fn exists(&self) -> napi::Result<bool> {
        if let Some(filter) = self.canonical_read_filter()? {
            return self.filter_handle(filter).exists();
        }
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.compatibility_exists_on_target(
                self.execution_target()?,
                &self.type_id,
                &self.filters,
                self.read_control(),
            ))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }
}

impl NodeProjectedModelManager {
    fn filter_iid(&self, iid: &Iid) -> napi::Result<NodeProjectedManagerFilter> {
        let mut filter = self.manager_filter()?;
        filter.filter = filter
            .filter
            .try_and_iid(&self.package.projection, iid)
            .map_err(napi_sdk_diagnostic)?;
        Ok(filter)
    }

    fn filter_handle(&self, filter: ProjectedManagerFilter) -> NodeProjectedManagerFilter {
        NodeProjectedManagerFilter {
            package: Arc::clone(&self.package),
            filter,
            target: self.target.clone(),
            runtime: Arc::clone(&self.runtime),
            successor_batch_marker: self.successor_batch_marker.clone(),
        }
    }

    fn canonical_read_filter(&self) -> napi::Result<Option<ProjectedManagerFilter>> {
        if self.filters.is_empty() {
            return self.manager_filter().map(|filter| Some(filter.filter));
        }
        if let Some(filter) = &self.closed_filter {
            return Ok(Some(filter.clone()));
        }
        if self.has_empty_iids {
            return ProjectedManagerFilter::try_new(&self.package.projection, self.type_id.clone())
                .and_then(|filter| filter.try_and_iids(&self.package.projection, &[]))
                .map(Some)
                .map_err(napi_sdk_diagnostic);
        }
        Ok(None)
    }

    fn prepare_returning_delete(
        &self,
        iids: Vec<String>,
    ) -> napi::Result<(ProjectedBatch, ProjectedBatchInvocationControl)> {
        ProjectedBatch::validate_binding_row_count(iids.len()).map_err(napi_sdk_diagnostic)?;
        let control =
            ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
        let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
            self.package.projection.as_ref(),
            self.type_id.clone(),
            ProjectedBatchOperation::Delete,
            &control,
        )
        .map_err(napi_sdk_diagnostic)?;
        let mut rows = reserved_projected_batch_rows(iids.len()).map_err(napi_sdk_diagnostic)?;
        for iid in iids {
            let row = ProjectedBatchRow::Delete { iid };
            budget
                .try_add_row(self.package.projection.as_ref(), &row)
                .map_err(napi_sdk_diagnostic)?;
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
        .map_err(napi_sdk_diagnostic)?;
        Ok((batch, control))
    }

    #[cfg(all(test, feature = "contract-test-adapter"))]
    fn delete_returning_envelopes(
        &self,
        iids: Vec<String>,
    ) -> napi::Result<Vec<NodeProjectedValueEnvelope>> {
        let (batch, control) = self.prepare_returning_delete(iids)?;
        self.execute_projected_batch(&batch, control, true, Vec::new(), |result| {
            let ProjectedBatchResult::Things(things) = result else {
                return Err(SdkExecutionDiagnostic::internal_failure());
            };
            let (things, input_indexes) = things.into_parts();
            let mut values = reserved_projected_batch_rows(things.len())?;
            for (thing, ordinal) in things.into_iter().zip(input_indexes) {
                values.push(
                    projected_envelope(Arc::clone(&self.package), thing).map_err(|_| {
                        ProjectedBatchExecutor::binding_materialization_failure(ordinal as u64)
                    })?,
                );
            }
            Ok(values)
        })
        .map_err(napi_sdk_diagnostic)
    }

    fn execute_single_projected<T, F>(
        &self,
        instance: ProjectedCreate,
        operation: ProjectedBatchOperation,
        iid: Option<String>,
        mapper: F,
    ) -> napi::Result<T>
    where
        F: FnOnce(
            type_bridge_orm::WriteResult<ProjectedThing>,
        ) -> Result<T, SdkExecutionDiagnostic>,
    {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let control =
            ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
        let row = match operation {
            ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put => {
                ProjectedBatchRow::Create(instance)
            }
            ProjectedBatchOperation::Update => ProjectedBatchRow::Update {
                iid: iid.ok_or_else(|| runtime_error("projected update omitted its IID"))?,
                replacement: instance,
            },
            ProjectedBatchOperation::Delete => {
                return Err(runtime_error("projected create cannot select deletion"));
            }
        };
        let batch = executor
            .single_controlled_write_batch(&self.type_id, operation, row, &control)
            .map_err(napi_sdk_diagnostic)?;
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| runtime_error("projected manager has no execution target"))?
            .as_target();
        self.runtime
            .block_on(executor.execute_single_write_on_target(target, &batch, control, mapper))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    fn write_single_materialized<'env>(
        &self,
        env: &Env,
        instance: ProjectedCreate,
        operation: ProjectedBatchOperation,
        iid: Option<String>,
    ) -> napi::Result<Object<'env>> {
        if self.package.projected_batch_materializer.is_none() {
            return Err(invalid_error(
                "native projection has no installed batch materializer",
            ));
        }
        let authority = NodeProjectedBatchAuthority::pending(Arc::clone(&self.package.projection));
        let guard = PendingBatchAuthority::new(Arc::clone(&authority));
        let output = self.execute_single_projected(instance, operation, iid, |result| {
            let result = BindingWriteAnswers::Single(result.into_items());
            self.materialize_projected_write_result(env, result, authority, true)
        })?;
        guard.finish();
        Ok(output)
    }

    #[cfg(test)]
    fn write_single_projected(
        &self,
        instance: ProjectedCreate,
        operation: ProjectedBatchOperation,
        iid: Option<String>,
    ) -> napi::Result<Vec<NodeProjectedValueEnvelope>> {
        self.execute_single_projected(instance, operation, iid, |result| {
            result
                .into_items()
                .into_iter()
                .enumerate()
                .map(|(ordinal, thing)| {
                    projected_envelope(Arc::clone(&self.package), thing).map_err(|_| {
                        ProjectedBatchExecutor::binding_materialization_failure(ordinal as u64)
                    })
                })
                .collect()
        })
    }

    fn write_many_projected<'env>(
        &self,
        env: &Env,
        row_count: u32,
        row_at: sys::napi_value,
        operation: ProjectedBatchOperation,
    ) -> napi::Result<Object<'env>> {
        let (batch, control) = capture_projected_batch(
            env.raw(),
            self.package.as_ref(),
            &self.type_id,
            operation,
            row_count,
            row_at,
        )?;
        self.execute_materialized_batch(env, &batch, control, false, false)
    }

    fn execute_materialized_batch<'env>(
        &self,
        env: &Env,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        returning_deleted: bool,
        single: bool,
    ) -> napi::Result<Object<'env>> {
        if batch.is_empty() {
            let items = frozen_empty_array(env)?;
            let output =
                projected_write_result_object(env.raw(), items.raw(), 0, Some(&[]), single)?;
            let output = unsafe { Object::from_napi_value(env.raw(), output) }?;
            self.execute_projected_batch(batch, control, returning_deleted, (), |_| {
                Err(SdkExecutionDiagnostic::internal_failure())
            })
            .map_err(napi_sdk_diagnostic)?;
            return Ok(output);
        }
        if self.package.projected_batch_materializer.is_none() {
            return Err(invalid_error(
                "native projection has no installed batch materializer",
            ));
        }
        let authority = NodeProjectedBatchAuthority::pending(Arc::clone(&self.package.projection));
        let authority_guard = PendingBatchAuthority::new(Arc::clone(&authority));
        let output = self
            .execute_projected_batch(batch, control, returning_deleted, None, |result| {
                self.materialize_projected_write_result(env, result.into(), authority, single)
                    .map(Some)
            })
            .map_err(napi_sdk_diagnostic)?;
        let output =
            output.ok_or_else(|| runtime_error("nonempty projected batch omitted its result"))?;
        authority_guard.finish();
        Ok(output)
    }

    fn materialize_projected_write_result<'env>(
        &self,
        env: &Env,
        result: BindingWriteAnswers,
        authority: Arc<NodeProjectedBatchAuthority>,
        single: bool,
    ) -> Result<Object<'env>, SdkExecutionDiagnostic> {
        let count = match &result {
            BindingWriteAnswers::Single(things) => things.len(),
            BindingWriteAnswers::Batch(ProjectedBatchResult::Things(things)) => things.count(),
            _ => return Err(SdkExecutionDiagnostic::internal_failure()),
        };
        let output = prepare_projected_batch_output(
            env,
            Arc::clone(&self.package),
            &self.type_id,
            count,
            authority,
        )
        .map_err(|_| materialization_failure(0))?;
        let output = output.materialize(result, single)?;
        let output = unsafe { Object::from_napi_value(env.raw(), output) }
            .map_err(|_| materialization_failure(0))?;
        Ok(output)
    }

    fn execute_projected_batch<T, F>(
        &self,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        returning_deleted: bool,
        empty: T,
        mapper: F,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(ProjectedBatchResult) -> Result<T, SdkExecutionDiagnostic>,
    {
        let executor = ProjectedBatchExecutor::new(self.package.projection.as_ref());
        let executor = if returning_deleted {
            executor.returning_deleted()
        } else {
            executor
        };
        let target = self
            .target
            .as_ref()
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let transaction = target.transaction().filter(|_| !batch.is_empty());
        let marker = if let Some(transaction) = transaction {
            let prior = self.runtime.block_on(transaction.lifecycle_state());
            let marker = self
                .successor_batch_marker
                .as_ref()
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            matches!(
                prior,
                TransactionContextState::Active | TransactionContextState::RolledBack
            )
            .then_some(marker)
        } else {
            None
        };
        let result = self.runtime.block_on(executor.execute_on_target_mapped(
            target.as_target(),
            batch,
            control,
            empty,
            |result| {
                if let Some(marker) = marker {
                    marker.store(true, Ordering::Release);
                }
                mapper(result)
            },
        ));
        if let (Some(transaction), Some(marker)) = (transaction, marker)
            && (result.is_ok()
                || self.runtime.block_on(transaction.lifecycle_state())
                    == TransactionContextState::RollbackOnly)
        {
            marker.store(true, Ordering::Release);
        }
        result.map_err(|error| {
            error.into_diagnostic(type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write)
        })
    }

    fn projected_create_input(
        &self,
        instance_json: &str,
        proofs: &[Option<NodeProjectedInputProof>],
    ) -> napi::Result<ProjectedCreate> {
        let instance = parse_wire(instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        let input = project_create_wire_with_proofs(
            self.package.as_ref(),
            &self.type_id,
            &instance,
            proofs,
        )
        .map_err(napi_sdk_diagnostic)?;
        input.validate_native_write().map_err(napi_sdk_diagnostic)?;
        Ok(input)
    }

    fn execution_target(&self) -> napi::Result<type_bridge_orm::QueryTarget<'_>> {
        self.target
            .as_ref()
            .map(OwnedQueryTarget::as_target)
            .ok_or_else(|| runtime_error("projected manager has no execution target"))
    }

    fn read_control(&self) -> ProjectedManagerFilterInvocationControl {
        ProjectedManagerFilterInvocationControl::capture(Default::default(), Default::default())
    }

    fn read_all_projected(&self) -> napi::Result<Vec<Arc<ProjectedThing>>> {
        if let Some(filter) = self.canonical_read_filter()? {
            return self.filter_handle(filter).execute_all();
        }
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.compatibility_all_on_target(
                self.execution_target()?,
                &self.type_id,
                &self.filters,
                self.read_control(),
            ))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    fn read_first_projected(&self) -> napi::Result<Option<Arc<ProjectedThing>>> {
        if (self.has_empty_iids
            || self
                .closed_filter
                .as_ref()
                .is_some_and(|filter| filter.exact_iids().is_some_and(|iids| iids.len() == 1)))
            && let Some(filter) = self.canonical_read_filter()?
        {
            return self.filter_handle(filter).execute_first();
        }
        let executor = ProjectedManagerFilterExecutor::new(self.package.projection.as_ref());
        self.runtime
            .block_on(executor.compatibility_first_on_target(
                self.execution_target()?,
                &self.type_id,
                &self.filters,
                self.read_control(),
            ))
            .map_err(|error| {
                error.into_diagnostic(
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
                )
            })
            .map_err(napi_sdk_diagnostic)
    }

    fn read_all_values(&self) -> napi::Result<Vec<ProjectedWire>> {
        self.read_all_projected()?
            .iter()
            .map(|thing| projected_thing_wire(self.package.as_ref(), thing.as_ref()))
            .collect()
    }

    fn descriptor(&self) -> napi::Result<TypeDescriptor> {
        self.package
            .projection
            .descriptor(&self.type_id)
            .cloned()
            .map_err(orm_error)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireForm {
    Complete,
    Reference,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProjectedWire {
    type_key: String,
    form: WireForm,
    iid: Option<String>,
    value: Option<ScalarWire>,
    values: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProjectedStructWire {
    type_key: String,
    values: BTreeMap<String, Option<ScalarWire>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderedManagerFilterEntry {
    name: String,
    value: Value,
}

#[cfg(test)]
fn projected_envelope(
    package: Arc<InstalledPackage>,
    thing: ProjectedThing,
) -> napi::Result<NodeProjectedValueEnvelope> {
    projected_envelope_arc(package, Arc::new(thing))
}

fn projected_envelope_arc(
    package: Arc<InstalledPackage>,
    thing: Arc<ProjectedThing>,
) -> napi::Result<NodeProjectedValueEnvelope> {
    let json = wire_json(&projected_thing_wire(package.as_ref(), thing.as_ref())?)?;
    Ok(NodeProjectedValueEnvelope {
        package,
        json,
        thing,
    })
}

fn install_authority_backed_projection(
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    schema_authority_json: &str,
) -> napi::Result<(RuntimeProjection, ManagedScopeId, VerifiedSchemaAuthority)> {
    let rejection = || projection_evidence_rejection(semantic_fingerprint_json);
    let runtime = decode_runtime_projection_verified(
        projection_json.as_bytes(),
        semantic_fingerprint_json.as_bytes(),
        projection_fingerprint_json.as_bytes(),
    )
    .map_err(|_| rejection())?;
    if runtime.target() != BindingTarget::TypeScript {
        return Err(rejection());
    }
    let authority = decode_schema_authority(
        schema_authority_json.as_bytes(),
        &schema_authority_capability_vocabulary(),
    )
    .map_err(|_| rejection())?;
    verify_projection_evidence(&authority, &runtime).map_err(|_| rejection())?;
    let managed_scope_id = authority.managed_scope().id().clone();
    Ok((runtime, managed_scope_id, authority))
}

fn projection_evidence_mismatch() -> Error {
    napi_sdk_diagnostic(SdkExecutionDiagnostic::projection_evidence_mismatch())
}

fn projection_evidence_rejection(semantic_fingerprint_json: &str) -> Error {
    let presence = if semantic_fingerprint_json.is_empty() {
        SdkProjectionEvidenceSlotPresence::Absent
    } else {
        SdkProjectionEvidenceSlotPresence::Present
    };
    napi_sdk_diagnostic(
        SdkExecutionDiagnostic::classify_detached_semantic_schema_fingerprint_rejection(presence),
    )
}

fn verify_legacy_typescript_projection_evidence(runtime: &RuntimeProjection) -> napi::Result<()> {
    if projection_uses_ordered_collections(runtime) {
        return Err(invalid_error(
            "ordered TypeScript runtime projections require compiled schema authority",
        ));
    }
    let emitter = TypeScriptEmitter::new();
    let resources = emitter.code_resources().map_err(diagnostic_error)?;
    if runtime.config().typescript_naming_policy()
        != ProjectionConfig::typescript().typescript_naming_policy()
        || runtime.generator_handlers() != emitter.generator_handlers()
        || runtime.code_resources() != resources
    {
        return Err(invalid_error(
            "legacy TypeScript runtime projection does not match the exact shipped handler and resource evidence",
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScalarWire {
    value_type: ValueTypeTag,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum GeneratedPackagePathWire {
    Type {
        #[serde(rename = "typeKey")]
        type_key: String,
    },
    Field {
        name: String,
    },
    Role {
        name: String,
    },
    Index {
        value: u32,
    },
}

fn generated_package_path(
    package: &InstalledPackage,
    path: &[GeneratedPackagePathWire],
) -> napi::Result<Vec<SdkDiagnosticPathSegment>> {
    if path.is_empty()
        || path.len() > MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS
        || !path.len().is_multiple_of(3)
    {
        return Err(invalid_error(
            "generated package mismatch path has an invalid shape",
        ));
    }
    let mut lowered = Vec::with_capacity(path.len());
    for chunk in path.as_chunks::<3>().0 {
        let GeneratedPackagePathWire::Type { type_key } = &chunk[0] else {
            return Err(invalid_error(
                "generated package mismatch path requires a projected type",
            ));
        };
        let id = manageable_type(package, type_key)?;
        let model = package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection model is absent"))?;
        let member = match &chunk[1] {
            GeneratedPackagePathWire::Field { name } => model
                .query_tokens()
                .fields()
                .values()
                .find(|field| field.target_name().as_str() == name)
                .map(|field| SdkDiagnosticPathSegment::Field(field.id().clone())),
            GeneratedPackagePathWire::Role { name } => model
                .query_tokens()
                .roles()
                .iter()
                .find(|(_, role)| role.target_name().as_str() == name)
                .map(|(role, _)| SdkDiagnosticPathSegment::Role(role.clone())),
            _ => None,
        }
        .ok_or_else(|| invalid_error("generated package mismatch member is not projected"))?;
        let GeneratedPackagePathWire::Index { value } = &chunk[2] else {
            return Err(invalid_error(
                "generated package mismatch member requires an index",
            ));
        };
        lowered.extend([
            SdkDiagnosticPathSegment::Type(id),
            member,
            SdkDiagnosticPathSegment::Index(u64::from(*value)),
        ]);
    }
    Ok(lowered)
}

fn project_create_wire(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> Result<ProjectedCreate, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            malformed_projected_create_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    let mut allowed = BTreeSet::new();
    let mut fields = Vec::with_capacity(model.create().fields().len());
    for field in model.create().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Field(field.token().clone()),
            ],
        )?;
        if field.multiplicity().container() == ProjectedContainer::Scalar && values.is_empty() {
            continue;
        }
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_attribute_wire(package, value).map_err(|diagnostic| {
                    rebase_sdk_diagnostic(
                        diagnostic,
                        [
                            projected_path(),
                            SdkDiagnosticPathSegment::Field(field.token().clone()),
                            SdkDiagnosticPathSegment::Index(projected_index(index)),
                        ],
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::with_capacity(model.create().roles().len());
    for (role_id, role) in model.create().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
            ],
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_reference_wire(
                    package,
                    role.players(),
                    value,
                    &[
                        projected_path(),
                        SdkDiagnosticPathSegment::Role(role_id.clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        roles.push((role_id.clone(), projected));
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_create_at([projected_path()]));
    }
    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles)
}

fn project_create_wire_with_proofs(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
    proofs: &[Option<NodeProjectedInputProof>],
) -> Result<ProjectedCreate, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            malformed_projected_create_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    let mut allowed = BTreeSet::new();
    let mut fields = Vec::with_capacity(model.create().fields().len());
    for field in model.create().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Field(field.token().clone()),
            ],
        )?;
        if field.multiplicity().container() == ProjectedContainer::Scalar && values.is_empty() {
            continue;
        }
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_attribute_wire(package, value).map_err(|diagnostic| {
                    rebase_sdk_diagnostic(
                        diagnostic,
                        [
                            projected_path(),
                            SdkDiagnosticPathSegment::Field(field.token().clone()),
                            SdkDiagnosticPathSegment::Index(projected_index(index)),
                        ],
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut proof_index = 0_usize;
    let mut roles = Vec::with_capacity(model.create().roles().len());
    for (role_id, role) in model.create().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
            ],
        )?;
        let mut projected = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let operation_path = [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
                SdkDiagnosticPathSegment::Index(projected_index(index)),
            ];
            let proof = proofs.get(proof_index).and_then(Option::as_ref);
            proof_index = proof_index.saturating_add(1);
            projected.push(project_reference_wire_with_input_proof(
                package,
                role.players(),
                value,
                &operation_path,
                proof,
            )?);
        }
        roles.push((role_id.clone(), projected));
    }
    if proof_index != proofs.len()
        || wire
            .values
            .keys()
            .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_create_at([projected_path()]));
    }
    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles)
}

fn project_thing_wire(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            generated_token_package_mismatch_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    if wire.value.is_some() {
        return Err(malformed_projected_hydration_at([projected_path()]));
    }

    let mut allowed = BTreeSet::new();
    let mut fields = Vec::<(OwnsFactId, Vec<ProjectedAttributeValue>)>::with_capacity(
        model.complete_read().fields().len(),
    );
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_hydration_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let path = [
            projected_path(),
            SdkDiagnosticPathSegment::Field(field.token().clone()),
        ];
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            path.iter().cloned(),
        )?;
        if field.multiplicity().container() == ProjectedContainer::Scalar && values.is_empty() {
            continue;
        }
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut value_path = path.to_vec();
                value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
                project_hydrated_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::<(RoleId, Vec<ProjectedRolePlayer>)>::with_capacity(
        model.complete_read().roles().len(),
    );
    for (role_id, role) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_hydration_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let path = [
            projected_path(),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
        ];
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            path.iter().cloned(),
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut value_path = path.to_vec();
                value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
                project_hydrated_role_player_wire(package, role, value, &value_path)
            })
            .collect::<Result<Vec<_>, _>>()?;
        roles.push((role_id.clone(), projected));
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_hydration_at([projected_path()]));
    }

    ProjectedThing::try_new(
        &package.projection,
        id.clone(),
        wire.iid.clone().unwrap_or_default(),
        fields,
        roles,
    )
}

fn projected_hydration_wire_items(
    value: Option<&Value>,
    multiplicity: ProjectedMultiplicity,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> Result<Vec<ProjectedWire>, SdkExecutionDiagnostic> {
    let path = path.into_iter().collect::<Vec<_>>();
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if multiplicity.container() == ProjectedContainer::Sequence => {
            values
                .iter()
                .map(|value| {
                    serde_json::from_value(value.clone())
                        .map_err(|_| malformed_projected_hydration_at(path.clone()))
                })
                .collect()
        }
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            serde_json::from_value(value.clone())
                .map(|value| vec![value])
                .map_err(|_| malformed_projected_hydration_at(path))
        }
        Some(_) => Err(malformed_projected_hydration_at(path)),
    }
}

fn project_hydrated_attribute_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_hydration_at(std::iter::empty()))?;
    let path = [SdkDiagnosticPathSegment::Type(id.clone())];
    if id.kind() != TypeKind::Attribute
        || wire.form != WireForm::Complete
        || wire.iid.is_some()
        || !wire.values.is_empty()
    {
        return Err(malformed_projected_hydration_at(path));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(path.clone()))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| malformed_projected_hydration_at(path.clone()))?;
    let scalar = wire
        .value
        .as_ref()
        .ok_or_else(|| malformed_projected_hydration_at(path.clone()))?;
    project_hydrated_attribute_scalar(&package.projection, id, scalar, ValueType::from(value_type))
}

fn project_hydrated_attribute_scalar(
    projection: &InstalledRuntimeProjection,
    id: TypeId,
    wire: &ScalarWire,
    expected: ValueType,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let value = scalar_to_ordered_attribute(wire, expected).map_err(|_| {
        wrong_hydrated_scalar_domain_at([SdkDiagnosticPathSegment::Type(id.clone())])
    })?;
    ProjectedAttributeValue::try_from_hydrated_attribute_value(projection, id, &value)
}

fn project_hydrated_reference_wire(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
    let form = match wire.form {
        WireForm::Complete => ProjectedModelForm::Complete,
        WireForm::Reference => ProjectedModelForm::Reference,
    };
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) || wire.value.is_some() {
        return Err(malformed_projected_hydration_at(
            operation_path.iter().cloned(),
        ));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(malformed_projected_hydration_at(
            operation_path.iter().cloned(),
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(operation_path.iter().cloned()))?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
        let mut key_path = operation_path.to_vec();
        key_path.extend([
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Field(key_id.clone()),
        ]);
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            read.multiplicity(),
            key_path.iter().cloned(),
        )?;
        for (index, value) in values.iter().enumerate() {
            let mut value_path = key_path.clone();
            value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
            keys.push((
                key_id.clone(),
                project_hydrated_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))?,
            ));
        }
    }
    ProjectedReference::try_new_for_hydration(&package.projection, id, wire.iid.clone(), keys)
        .map_err(|diagnostic| {
            rebase_hydration_diagnostic(diagnostic, operation_path.iter().cloned())
        })
}

fn project_hydrated_role_player_wire(
    package: &InstalledPackage,
    read_role: &type_bridge_contract::projection::ReadRoleProjection,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
) -> Result<ProjectedRolePlayer, SdkExecutionDiagnostic> {
    let form = match wire.form {
        WireForm::Complete => ProjectedModelForm::Complete,
        WireForm::Reference => ProjectedModelForm::Reference,
    };
    let reference =
        project_hydrated_reference_wire(package, read_role.players(), wire, operation_path)?;
    if form == ProjectedModelForm::Reference {
        return ProjectedRolePlayer::try_new_reference_for_hydration(
            &package.projection,
            read_role,
            reference,
        )
        .map_err(|diagnostic| {
            rebase_hydration_diagnostic(diagnostic, operation_path.iter().cloned())
        });
    }
    let id = reference.type_id().clone();
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(operation_path.iter().cloned()))?;
    let mut fields = Vec::with_capacity(model.complete_read().fields().len());
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
        let mut field_path = operation_path.to_vec();
        field_path.extend([
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Field(field.token().clone()),
        ]);
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            field_path.iter().cloned(),
        )?;
        if field.multiplicity().container() == ProjectedContainer::Scalar && values.is_empty() {
            continue;
        }
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut value_path = field_path.clone();
                value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
                project_hydrated_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }
    ProjectedRolePlayer::try_new_complete_for_hydration(
        &package.projection,
        read_role,
        reference,
        fields,
    )
    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, operation_path.iter().cloned()))
}

fn projected_wire_items(
    value: Option<&Value>,
    multiplicity: ProjectedMultiplicity,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> Result<Vec<ProjectedWire>, SdkExecutionDiagnostic> {
    let path = path.into_iter().collect::<Vec<_>>();
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if multiplicity.container() == ProjectedContainer::Sequence => {
            values
                .iter()
                .map(|value| {
                    serde_json::from_value(value.clone())
                        .map_err(|_| malformed_projected_create_at(path.clone()))
                })
                .collect()
        }
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            serde_json::from_value(value.clone())
                .map(|value| vec![value])
                .map_err(|_| malformed_projected_create_at(path))
        }
        Some(_) => Err(malformed_projected_create_at(path)),
    }
}

fn project_attribute_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_create_at(std::iter::empty()))?;
    let path = [SdkDiagnosticPathSegment::Type(id.clone())];
    if id.kind() != TypeKind::Attribute
        || wire.form != WireForm::Complete
        || !wire.values.is_empty()
    {
        return Err(malformed_projected_create_at(path));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(path.clone()))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| malformed_projected_create_at(path.clone()))?;
    let scalar = wire
        .value
        .as_ref()
        .ok_or_else(|| malformed_projected_create_at(path))?;
    project_attribute_scalar(&package.projection, id, scalar, ValueType::from(value_type))
}

fn project_attribute_scalar(
    projection: &InstalledRuntimeProjection,
    id: TypeId,
    wire: &ScalarWire,
    expected: ValueType,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let value = scalar_to_ordered_attribute(wire, expected)
        .map_err(|_| wrong_scalar_domain_at([SdkDiagnosticPathSegment::Type(id.clone())]))?;
    ProjectedAttributeValue::try_from_attribute_value(projection, id, &value)
}

fn project_reference_wire(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_create_at(operation_path.iter().cloned()))?;
    let form = match wire.form {
        WireForm::Complete => ProjectedModelForm::Complete,
        WireForm::Reference => ProjectedModelForm::Reference,
    };
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation)
        || wire.value.is_some()
        || !allowed_players
            .iter()
            .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(malformed_projected_create_at(
            operation_path.iter().cloned(),
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
        let mut key_path = operation_path.to_vec();
        key_path.extend([
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Field(key_id.clone()),
        ]);
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            read.multiplicity(),
            key_path.iter().cloned(),
        )?;
        for (index, value) in values.iter().enumerate() {
            let mut value_path = key_path.clone();
            value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
            keys.push((
                key_id.clone(),
                project_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_sdk_diagnostic(diagnostic, value_path))?,
            ));
        }
    }
    ProjectedReference::try_new(&package.projection, id, wire.iid.clone(), keys).map_err(
        |diagnostic| {
            let mut path = operation_path.to_vec();
            path.extend(diagnostic.path().iter().cloned());
            rebase_sdk_diagnostic(diagnostic, path)
        },
    )
}

#[cfg(test)]
fn project_reference_wire_with_proof(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
    proof: Option<&NodeProjectedFacadeProof>,
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let proof = proof.map(|proof| NodeProjectedInputProof::Single(proof.proof.clone()));
    project_reference_wire_with_input_proof(
        package,
        allowed_players,
        wire,
        operation_path,
        proof.as_ref(),
    )
}

fn project_reference_wire_with_input_proof(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
    proof: Option<&NodeProjectedInputProof>,
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let visible = project_reference_wire(package, allowed_players, wire, operation_path)?;
    let Some(proof) = proof else {
        return Ok(visible);
    };
    let retained = proof
        .reference(package)
        .map_err(|diagnostic| rebase_sdk_diagnostic(diagnostic, operation_path.iter().cloned()))?;
    if visible != retained {
        return Err(malformed_projected_create_at(
            operation_path.iter().cloned(),
        ));
    }
    Ok(retained)
}

fn rebase_sdk_diagnostic(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    let rebased = match diagnostic.category() {
        SdkDiagnosticCategory::InvalidInput => {
            SdkExecutionDiagnostic::invalid_input(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::ResourceLimit => {
            SdkExecutionDiagnostic::resource_limit(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::Integrity => {
            SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message())
        }
        _ => return malformed_projected_create_at(path),
    };
    let rebased = append_sdk_path(rebased, path);
    diagnostic
        .details()
        .iter()
        .fold(rebased, |rebased, (name, detail)| {
            rebased
                .try_with_detail(name.clone(), detail.clone())
                .expect("common projected-reference details fit the SDK diagnostic contract")
        })
}

fn rebase_hydration_diagnostic(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    let rebased = match diagnostic.category() {
        SdkDiagnosticCategory::ResourceLimit => {
            SdkExecutionDiagnostic::resource_limit(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::Integrity => {
            SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message())
        }
        _ => return malformed_projected_hydration_at(path),
    };
    let rebased = append_sdk_path(rebased, path);
    diagnostic
        .details()
        .iter()
        .fold(rebased, |rebased, (name, detail)| {
            rebased
                .try_with_detail(name.clone(), detail.clone())
                .expect("common projected-hydration details fit the SDK diagnostic contract")
        })
}

fn wrong_scalar_domain_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ),
        path,
    )
}

fn malformed_projected_create_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("malformed_projected_create")
                .expect("static generated-create code is canonical"),
            SdkDiagnosticMessage::new("generated create payload has an invalid projected shape")
                .expect("static generated-create message is canonical"),
        ),
        path,
    )
}

fn wrong_hydrated_scalar_domain_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ),
        path,
    )
}

fn malformed_projected_hydration_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("malformed_projected_hydration")
                .expect("static generated-hydration code is canonical"),
            SdkDiagnosticMessage::new("generated hydration payload has an invalid projected shape")
                .expect("static generated-hydration message is canonical"),
        ),
        path,
    )
}

fn generated_token_package_mismatch_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::generated_token_package_mismatch(),
        path,
    )
}

fn append_sdk_path(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    path.into_iter().fold(diagnostic, |diagnostic, segment| {
        diagnostic
            .try_at(segment)
            .expect("a generated create operation path fits the SDK diagnostic contract")
    })
}

fn projected_index(index: usize) -> u64 {
    u64::try_from(index).expect("a projected collection index fits the SDK diagnostic contract")
}

fn manageable_type(package: &InstalledPackage, type_key: &str) -> napi::Result<TypeId> {
    let id = type_id_from_key(type_key)?;
    package
        .projection
        .descriptor(&id)
        .map_err(|_| invalid_error("attribute and struct tokens do not expose CRUD managers"))?;
    Ok(id)
}

fn parse_wire(value: &str) -> napi::Result<ProjectedWire> {
    serde_json::from_str(value)
        .map_err(|error| invalid_error(format!("invalid projected value wire: {error}")))
}

fn nested_wire(value: &Value) -> napi::Result<ProjectedWire> {
    serde_json::from_value(value.clone())
        .map_err(|error| invalid_error(format!("invalid nested projected value wire: {error}")))
}

fn type_id_from_key(value: &str) -> napi::Result<TypeId> {
    let id: TypeId = serde_json::from_str(value)
        .map_err(|error| invalid_error(format!("invalid canonical type key: {error}")))?;
    let canonical = to_canonical_json(&id).map_err(diagnostic_error)?;
    if canonical.as_slice() != value.as_bytes() {
        return Err(invalid_error("projected type key is not canonical JSON"));
    }
    Ok(id)
}

fn canonical_type_key(id: &TypeId) -> napi::Result<String> {
    String::from_utf8(to_canonical_json(id).map_err(diagnostic_error)?)
        .map_err(|error| runtime_error(error.to_string()))
}

fn ensure_root_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
    expected: &TypeId,
) -> napi::Result<()> {
    if wire.form != WireForm::Complete || type_id_from_key(&wire.type_key)? != *expected {
        return Err(invalid_error(
            "insert requires the manager's exact complete projected model",
        ));
    }
    if wire.value.is_some() {
        return Err(invalid_error(
            "entity and relation wires cannot carry scalar values",
        ));
    }
    ensure_wire_members(package, expected, wire)
}

fn ensure_wire_members(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> napi::Result<()> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| invalid_error("projected wire references an unknown model"))?;
    let mut allowed = BTreeSet::new();
    match wire.form {
        WireForm::Complete => {
            for field in model.create().fields() {
                let token = model
                    .query_tokens()
                    .fields()
                    .get(field.token())
                    .ok_or_else(|| runtime_error("projected create field has no query token"))?;
                allowed.insert(token.target_name().as_str());
            }
            for role in model.create().roles().values() {
                let token = model
                    .query_tokens()
                    .roles()
                    .get(role.role())
                    .ok_or_else(|| runtime_error("projected create role has no query token"))?;
                allowed.insert(token.target_name().as_str());
            }
        }
        WireForm::Reference => {
            for field in model.reference_read().key_fields() {
                let token =
                    model.query_tokens().fields().get(field).ok_or_else(|| {
                        runtime_error("projected reference key has no query token")
                    })?;
                allowed.insert(token.target_name().as_str());
            }
        }
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(invalid_error(
            "projected wire contains an unprojected member",
        ));
    }
    Ok(())
}

struct LoweredManagerFilters {
    expressions: Vec<DynamicExpr>,
    closed: Option<ProjectedManagerFilter>,
    empty_iids: bool,
}

fn lower_filter_values(
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    filters: Vec<OrderedManagerFilterEntry>,
    mut closed: Option<ProjectedManagerFilter>,
) -> napi::Result<LoweredManagerFilters> {
    let mut empty_iids = false;
    let mut lowered = Vec::with_capacity(filters.len());
    for OrderedManagerFilterEntry { name: key, value } in filters {
        if matches!(key.as_str(), "iid" | "_iid" | "iid__eq" | "_iid__eq") {
            let iid = projected_filter_iid(&value)?;
            if let Some(filter) = closed.take() {
                closed = Some(
                    filter
                        .try_and_iid(&package.projection, &iid)
                        .map_err(napi_sdk_diagnostic)?,
                );
            }
            lowered.push(DynamicExpr::Iid {
                iid: iid.into_string(),
            });
            continue;
        }
        if matches!(key.as_str(), "iid__in" | "_iid__in") {
            let values = value.as_array().ok_or_else(|| {
                invalid_error("generated manager iid__in lookup requires an array")
            })?;
            empty_iids |= values.is_empty();
            let iids = values
                .iter()
                .map(projected_filter_iid)
                .collect::<napi::Result<Vec<_>>>()?;
            lowered.push(DynamicExpr::Or {
                exprs: iids
                    .iter()
                    .map(|iid| DynamicExpr::Iid {
                        iid: iid.as_str().to_owned(),
                    })
                    .collect(),
            });
            if let Some(filter) = closed.take() {
                closed = Some(
                    filter
                        .try_and_owned_iids(&package.projection, iids)
                        .map_err(napi_sdk_diagnostic)?,
                );
            }
            continue;
        }
        // Generated target names may themselves contain a recognised lookup
        // suffix. Prefer the suffix only when its prefix is also a projected
        // field; `scoreGte__eq` selects equality on the literal `scoreGte`.
        let has_field = |name: &str| {
            descriptors
                .iter()
                .any(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
        };
        let resolved = resolve_generated_manager_lookup(&key, has_field);
        let field_name = resolved.field_name();
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == field_name || descriptor.attr_name == field_name
            })
            .ok_or_else(|| {
                invalid_error(format!("unknown generated manager filter {field_name:?}"))
            })?;
        let lookup = resolved
            .operator()
            .map_err(|error| napi_sdk_diagnostic(error.into()))?;
        if lookup.requires_text() && descriptor.value_type != ValueType::String {
            return Err(invalid_error(format!(
                "unsupported generated manager lookup {:?} for non-string field {field_name:?}",
                resolved.lookup()
            )));
        }
        let operator = match lookup {
            ManagerLookup::IsNull => {
                closed = None;
                let is_null = value.as_bool().ok_or_else(|| {
                    invalid_error("generated manager isnull lookup requires a boolean")
                })?;
                lowered.push(DynamicExpr::IsNull {
                    attr_name: descriptor.attr_name.clone(),
                    is_null,
                });
                continue;
            }
            ManagerLookup::In => {
                closed = None;
                let values = value.as_array().ok_or_else(|| {
                    invalid_error("generated manager in lookup requires an array")
                })?;
                if values.is_empty() {
                    return Err(invalid_error(
                        "generated manager in lookup requires at least one value",
                    ));
                }
                lowered.push(DynamicExpr::Or {
                    exprs: values
                        .iter()
                        .map(|value| {
                            Ok(DynamicExpr::Compare {
                                attr_name: descriptor.attr_name.clone(),
                                operator: DynamicComparisonOp::Eq,
                                value: projected_filter_attribute_value(
                                    package, descriptor, field_name, value,
                                )?,
                            })
                        })
                        .collect::<napi::Result<Vec<_>>>()?,
                });
                continue;
            }
            ManagerLookup::Eq => DynamicComparisonOp::Eq,
            ManagerLookup::Ne => DynamicComparisonOp::Neq,
            ManagerLookup::Gt => DynamicComparisonOp::Gt,
            ManagerLookup::Gte => DynamicComparisonOp::Gte,
            ManagerLookup::Lt => DynamicComparisonOp::Lt,
            ManagerLookup::Lte => DynamicComparisonOp::Lte,
            ManagerLookup::Contains => DynamicComparisonOp::Contains,
            ManagerLookup::StartsWith => DynamicComparisonOp::StartsWith,
            ManagerLookup::EndsWith => DynamicComparisonOp::EndsWith,
            ManagerLookup::Regex => DynamicComparisonOp::Like,
        };
        let literal = projected_filter_attribute_value(package, descriptor, field_name, &value)?;
        closed = narrow_closed_filter(package, descriptor, lookup, &literal, closed)?;
        lowered.push(DynamicExpr::Compare {
            attr_name: descriptor.attr_name.clone(),
            operator,
            value: literal,
        });
    }
    Ok(LoweredManagerFilters {
        expressions: lowered,
        closed,
        empty_iids,
    })
}

fn narrow_closed_filter(
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    lookup: ManagerLookup,
    literal: &AttributeValue,
    base: Option<ProjectedManagerFilter>,
) -> napi::Result<Option<ProjectedManagerFilter>> {
    let Some(base) = base else { return Ok(None) };
    let Ok(comparison) = ProjectedManagerComparison::try_from(lookup) else {
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
    let model = base.model();
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
                .find(|field| field.target_name().as_str() == descriptor.field_name)
        })
        .ok_or_else(|| runtime_error("projected manager field token is absent"))?;
    let attribute = package.type_by_label(&descriptor.attr_name)?;
    let projected = ProjectedAttributeValue::try_from_attribute_value(
        &package.projection,
        attribute.clone(),
        literal,
    )
    .map_err(napi_sdk_diagnostic)?;
    base.try_and(
        &package.projection,
        &ProjectedTokenIdentity::Field {
            owner: model.clone(),
            field: field.id().clone(),
        },
        comparison,
        &projected,
    )
    .map(Some)
    .map_err(napi_sdk_diagnostic)
}

fn projected_filter_attribute_value(
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    field_name: &str,
    value: &Value,
) -> napi::Result<AttributeValue> {
    let wrapper = nested_wire(value)?;
    let expected = package.type_by_label(&descriptor.attr_name)?;
    ensure_wire_members(package, expected, &wrapper)?;
    if expected.kind() != TypeKind::Attribute
        || wrapper.form != WireForm::Complete
        || type_id_from_key(&wrapper.type_key)? != *expected
        || wrapper.iid.is_some()
        || !wrapper.values.is_empty()
    {
        return Err(invalid_error(format!(
            "generated manager filter {field_name:?} requires its exact attribute wrapper"
        )));
    }
    let scalar = wrapper.value.as_ref().ok_or_else(|| {
        invalid_error("complete generated manager filter wrapper has no scalar value")
    })?;
    scalar_to_attribute(scalar, descriptor.value_type)
}

fn projected_filter_iid(value: &Value) -> napi::Result<Iid> {
    let iid = value
        .as_str()
        .ok_or_else(|| invalid_error("generated manager IID wire requires strings"))?;
    Iid::new(iid).map_err(diagnostic_error)
}

fn match_attributes(
    package: &InstalledPackage,
    id: &TypeId,
    hydrated: &[HydratedAttribute],
) -> napi::Result<DynamicAttributeMap> {
    let descriptor = package.projection.descriptor(id).map_err(orm_error)?;
    let descriptors = match &descriptor {
        TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
    };
    let mut attributes = Vec::new();
    for attribute in hydrated {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == attribute.field().name)
            .ok_or_else(|| {
                runtime_error("validated match field is outside the installed projection")
            })?;
        attributes.extend(
            attribute
                .values()
                .iter()
                .cloned()
                .map(|value| (descriptor.attr_name.clone(), value)),
        );
    }
    Ok(attributes)
}

fn attribute_json_value(value: &AttributeValue) -> Value {
    match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => Value::String(value.clone()),
        AttributeValue::Long(value) => serde_json::json!(value),
        AttributeValue::Double(value) => serde_json::json!(value),
        AttributeValue::Boolean(value) => serde_json::json!(value),
    }
}

fn write_json_string(output: &mut FallibleJsonWriter, value: &str) -> fmt::Result {
    output.write_char('"')?;
    let bytes = value.as_bytes();
    let mut start = 0_usize;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let escaped = match byte {
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            b'\x08' => Some("\\b"),
            b'\x0c' => Some("\\f"),
            b'\n' => Some("\\n"),
            b'\r' => Some("\\r"),
            b'\t' => Some("\\t"),
            0x00..=0x1f => None,
            _ => continue,
        };
        if start < index {
            output.write_str(&value[start..index])?;
        }
        if let Some(escaped) = escaped {
            output.write_str(escaped)?;
        } else {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let escaped = [
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0x0f)],
            ];
            // SAFETY: the fixed JSON escape alphabet is ASCII.
            output.write_str(unsafe { std::str::from_utf8_unchecked(&escaped) })?;
        }
        start = index.saturating_add(1);
    }
    if start < value.len() {
        output.write_str(&value[start..])?;
    }
    output.write_char('"')
}

fn projected_type_key_cache<'a>(
    package: &'a InstalledPackage,
    id: &TypeId,
) -> Result<&'a ProjectedTypeKeyCache, fmt::Error> {
    package.projected_type_keys.get(id).ok_or(fmt::Error)
}

fn write_projected_wire_prefix(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    id: &TypeId,
    form: ProjectedModelForm,
    iid: Option<&str>,
) -> fmt::Result {
    let type_key = projected_type_key_cache(package, id)?;
    output.write_str("{\"typeKey\":")?;
    output.write_str(&type_key.json_string)?;
    output.write_str(",\"form\":")?;
    output.write_str(match form {
        ProjectedModelForm::Complete => "\"complete\"",
        ProjectedModelForm::Reference => "\"reference\"",
    })?;
    output.write_str(",\"iid\":")?;
    match iid {
        Some(iid) => write_json_string(output, iid)?,
        None => output.write_str("null")?,
    }
    Ok(())
}

fn write_fractional_nanoseconds(output: &mut FallibleJsonWriter, nanosecond: u32) -> fmt::Result {
    if nanosecond == 0 {
        return Ok(());
    }
    let mut digits = 9_u32;
    let mut trimmed = nanosecond;
    while trimmed.is_multiple_of(10) {
        trimmed /= 10;
        digits -= 1;
    }
    output.write_char('.')?;
    let mut divisor = 100_000_000_u32;
    for _ in 0..digits {
        output.write_char(char::from(b'0' + ((nanosecond / divisor) % 10) as u8))?;
        divisor /= 10;
    }
    Ok(())
}

fn write_canonical_time(
    output: &mut FallibleJsonWriter,
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
) -> fmt::Result {
    write!(output, "{hour:02}:{minute:02}:{second:02}")?;
    write_fractional_nanoseconds(output, nanosecond)
}

fn write_canonical_datetime(
    output: &mut FallibleJsonWriter,
    value: CanonicalDateTime,
) -> fmt::Result {
    let (year, month, day) = value.date().components();
    write_javascript_date(output, year, month, day)?;
    output.write_char('T')?;
    let (hour, minute, second, nanosecond) = value.time().components();
    write_canonical_time(output, hour, minute, second, nanosecond)
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn adjacent_date(year: i32, month: u8, day: u8, delta: i32) -> (i32, u8, u8) {
    debug_assert!(matches!(delta, -1..=1));
    match delta {
        -1 if day > 1 => (year, month, day - 1),
        -1 if month > 1 => {
            let month = month - 1;
            (year, month, days_in_month(year, month))
        }
        -1 => (year - 1, 12, 31),
        1 if day < days_in_month(year, month) => (year, month, day + 1),
        1 if month < 12 => (year, month + 1, 1),
        1 => (year + 1, 1, 1),
        _ => (year, month, day),
    }
}

fn write_javascript_date(
    output: &mut FallibleJsonWriter,
    year: i32,
    month: u8,
    day: u8,
) -> fmt::Result {
    // ECMAScript's date-time string format uses four digits in the ordinary
    // range and a signed six-digit extended year outside it.
    if (0..=9_999).contains(&year) {
        write!(output, "{year:04}")?;
    } else {
        let sign = if year < 0 { '-' } else { '+' };
        write!(
            output,
            "{sign}{absolute:06}",
            absolute = year.unsigned_abs()
        )?;
    }
    write!(output, "-{month:02}-{day:02}")
}

fn write_datetime_tz_evidence(
    output: &mut FallibleJsonWriter,
    value: &CanonicalDateTimeTz,
) -> fmt::Result {
    // JavaScript Date does not accept IANA suffixes or second-precision UTC
    // offsets. Preserve the exact instant by normalizing the already-resolved
    // local value and effective offset to UTC before crossing the callback.
    let local = value.local();
    let (year, month, day) = local.date().components();
    let (hour, minute, second, nanosecond) = local.time().components();
    let local_seconds = i32::from(hour) * 3_600 + i32::from(minute) * 60 + i32::from(second);
    let utc_seconds = local_seconds - value.effective_offset_seconds();
    let day_delta = utc_seconds.div_euclid(86_400);
    debug_assert!(matches!(day_delta, -1..=1));
    let seconds = utc_seconds.rem_euclid(86_400);
    let (year, month, day) = adjacent_date(year, month, day, day_delta);
    write_javascript_date(output, year, month, day)?;
    output.write_char('T')?;
    write_canonical_time(
        output,
        (seconds / 3_600) as u8,
        (seconds % 3_600 / 60) as u8,
        (seconds % 60) as u8,
        nanosecond,
    )?;
    output.write_char('Z')
}

fn write_canonical_duration(
    output: &mut FallibleJsonWriter,
    value: CanonicalDuration,
) -> fmt::Result {
    let (negative, months, days, seconds, nanosecond) = value.components();
    if negative {
        output.write_char('-')?;
    }
    output.write_char('P')?;
    if months != 0 {
        write!(output, "{months}M")?;
    }
    if days != 0 {
        write!(output, "{days}D")?;
    }
    if seconds != 0 || nanosecond != 0 || (months == 0 && days == 0) {
        write!(output, "T{seconds}")?;
        write_fractional_nanoseconds(output, nanosecond)?;
        output.write_char('S')?;
    }
    Ok(())
}

fn write_projected_scalar(output: &mut FallibleJsonWriter, value: &CanonicalValue) -> fmt::Result {
    output.write_str("{\"valueType\":")?;
    write_json_string(output, value.value_type().as_str())?;
    output.write_str(",\"value\":")?;
    match value {
        CanonicalValue::String(value) => write_json_string(output, value.as_str())?,
        CanonicalValue::Long(value) => {
            output.write_char('"')?;
            write!(output, "{value}")?;
            output.write_char('"')?;
        }
        CanonicalValue::Double(value) => {
            // Canonical doubles are finite. Rust's direct finite-f64 formatter
            // uses the shortest round-trippable representation without an
            // intermediate allocation or an io::Error payload.
            write!(output, "{}", value.get())?;
        }
        CanonicalValue::Boolean(value) => {
            output.write_str(if *value { "true" } else { "false" })?;
        }
        CanonicalValue::Date(value) => {
            output.write_char('"')?;
            let (year, month, day) = value.components();
            write_javascript_date(output, year, month, day)?;
            output.write_char('"')?;
        }
        CanonicalValue::DateTime(value) => {
            output.write_char('"')?;
            write_canonical_datetime(output, *value)?;
            output.write_char('"')?;
        }
        CanonicalValue::DateTimeTz(value) => {
            output.write_char('"')?;
            write_datetime_tz_evidence(output, value)?;
            output.write_char('"')?;
        }
        CanonicalValue::Decimal(value) => write_json_string(output, value.as_str())?,
        CanonicalValue::Duration(value) => {
            output.write_char('"')?;
            write_canonical_duration(output, *value)?;
            output.write_char('"')?;
        }
    }
    output.write_char('}')
}

pub(crate) fn canonical_plan_scalar_rows_json(
    rows: &[Vec<CanonicalValue>],
) -> napi::Result<String> {
    let mut output = FallibleJsonWriter::default();
    let write = |output: &mut FallibleJsonWriter| -> fmt::Result {
        output.write_char('[')?;
        for (ordinal, row) in rows.iter().enumerate() {
            if ordinal != 0 {
                output.write_char(',')?;
            }
            output.write_char('[')?;
            for (column, value) in row.iter().enumerate() {
                if column != 0 {
                    output.write_char(',')?;
                }
                write_projected_scalar(output, value)?;
            }
            output.write_char(']')?;
        }
        output.write_char(']')
    };
    write(&mut output)
        .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))?;
    String::from_utf8(output.bytes)
        .map_err(|_| napi_sdk_diagnostic(ProjectedBatch::binding_allocation_failure()))
}

fn write_projected_attribute_wire(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    value: &ProjectedAttributeValue,
) -> fmt::Result {
    write_projected_wire_prefix(
        output,
        package,
        value.attribute_type(),
        ProjectedModelForm::Complete,
        None,
    )?;
    output.write_str(",\"value\":")?;
    write_projected_scalar(output, value.value())?;
    output.write_str(",\"values\":{}}")
}

fn projected_cardinality_valid(multiplicity: ProjectedMultiplicity, count: usize) -> bool {
    let Ok(count) = u64::try_from(count) else {
        return false;
    };
    let cardinality = multiplicity.cardinality();
    count >= cardinality.min() && cardinality.max().is_none_or(|maximum| count <= maximum)
}

fn write_projected_attribute_member(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    values: &[ProjectedAttributeValue],
    multiplicity: ProjectedMultiplicity,
) -> fmt::Result {
    if !projected_cardinality_valid(multiplicity, values.len()) {
        return Err(fmt::Error);
    }
    match multiplicity.container() {
        ProjectedContainer::Scalar => match values {
            [] => output.write_str("null"),
            [value] => write_projected_attribute_wire(output, package, value),
            _ => Err(fmt::Error),
        },
        ProjectedContainer::Sequence => {
            output.write_char('[')?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.write_char(',')?;
                }
                write_projected_attribute_wire(output, package, value)?;
            }
            output.write_char(']')
        }
    }
}

fn write_projected_fields(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    id: &TypeId,
    fields: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    reference: bool,
    first: &mut bool,
) -> fmt::Result {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or(fmt::Error)?;
    let selected_count = model
        .complete_read()
        .fields()
        .iter()
        .filter(|field| !reference || model.reference_read().key_fields().contains(field.token()))
        .count();
    if fields.len() != selected_count {
        return Err(fmt::Error);
    }
    for read in
        model.complete_read().fields().iter().filter(|field| {
            !reference || model.reference_read().key_fields().contains(field.token())
        })
    {
        let token = model
            .query_tokens()
            .fields()
            .get(read.token())
            .ok_or(fmt::Error)?;
        let values = fields.get(read.token()).ok_or(fmt::Error)?;
        if !*first {
            output.write_char(',')?;
        }
        *first = false;
        write_json_string(output, token.target_name().as_str())?;
        output.write_char(':')?;
        write_projected_attribute_member(output, package, values, read.multiplicity())?;
    }
    Ok(())
}

fn write_projected_reference_fields(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    reference: &ProjectedReference,
    first: &mut bool,
) -> fmt::Result {
    let model = package
        .projection
        .projection()
        .models()
        .get(reference.type_id())
        .ok_or(fmt::Error)?;
    if reference.keys().len() != model.reference_read().key_fields().len() {
        return Err(fmt::Error);
    }
    for field_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or(fmt::Error)?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == field_id)
            .ok_or(fmt::Error)?;
        let value = reference.keys().get(field_id).ok_or(fmt::Error)?;
        if !*first {
            output.write_char(',')?;
        }
        *first = false;
        write_json_string(output, token.target_name().as_str())?;
        output.write_char(':')?;
        write_projected_attribute_member(
            output,
            package,
            std::slice::from_ref(value),
            read.multiplicity(),
        )?;
    }
    Ok(())
}

fn write_projected_role_player_wire(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    player: &ProjectedRolePlayer,
) -> fmt::Result {
    let form = player.exact_form().ok_or(fmt::Error)?;
    write_projected_wire_prefix(output, package, player.type_id(), form, Some(player.iid()))?;
    output.write_str(",\"value\":null,\"values\":{")?;
    let mut first = true;
    match form {
        ProjectedModelForm::Complete => write_projected_fields(
            output,
            package,
            player.type_id(),
            player.fields(),
            false,
            &mut first,
        )?,
        ProjectedModelForm::Reference => {
            write_projected_reference_fields(output, package, player.reference(), &mut first)?;
        }
    }
    output.write_str("}}")
}

fn write_projected_role_member(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    players: &[ProjectedRolePlayer],
    multiplicity: ProjectedMultiplicity,
) -> fmt::Result {
    if !projected_cardinality_valid(multiplicity, players.len()) {
        return Err(fmt::Error);
    }
    match multiplicity.container() {
        ProjectedContainer::Scalar => match players {
            [] => output.write_str("null"),
            [player] => write_projected_role_player_wire(output, package, player),
            _ => Err(fmt::Error),
        },
        ProjectedContainer::Sequence => {
            output.write_char('[')?;
            for (index, player) in players.iter().enumerate() {
                if index != 0 {
                    output.write_char(',')?;
                }
                write_projected_role_player_wire(output, package, player)?;
            }
            output.write_char(']')
        }
    }
}

fn write_projected_thing_json(
    output: &mut FallibleJsonWriter,
    package: &InstalledPackage,
    thing: &ProjectedThing,
) -> fmt::Result {
    write_projected_wire_prefix(
        output,
        package,
        thing.type_id(),
        ProjectedModelForm::Complete,
        Some(thing.iid()),
    )?;
    output.write_str(",\"value\":null,\"values\":{")?;
    let mut first = true;
    write_projected_fields(
        output,
        package,
        thing.type_id(),
        thing.fields(),
        false,
        &mut first,
    )?;
    let model = package
        .projection
        .projection()
        .models()
        .get(thing.type_id())
        .ok_or(fmt::Error)?;
    if thing.roles().len() != model.complete_read().roles().len() {
        return Err(fmt::Error);
    }
    for (role_id, read) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or(fmt::Error)?;
        let players = thing.roles().get(role_id).ok_or(fmt::Error)?;
        if !first {
            output.write_char(',')?;
        }
        first = false;
        write_json_string(output, token.target_name().as_str())?;
        output.write_char(':')?;
        write_projected_role_member(output, package, players, read.multiplicity())?;
    }
    output.write_str("}}")
}

fn projected_create_wire(
    package: &InstalledPackage,
    create: &ProjectedCreate,
) -> napi::Result<ProjectedWire> {
    create
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let id = create.type_id();
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("projected create model is absent"))?;
    let mut values = BTreeMap::new();
    for field in model.create().fields() {
        if !create.field_is_present(field.token()) {
            continue;
        }
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| runtime_error("projected create field has no query token"))?;
        let members = create
            .fields()
            .get(field.token())
            .map_or_else(|| &[][..], Vec::as_slice);
        let wires = members
            .iter()
            .map(|value| projected_attribute_value_wire(package, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(wires, field.multiplicity())?,
        );
    }
    for (role_id, role) in model.create().roles() {
        if !create.role_is_present(role_id) {
            continue;
        }
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| runtime_error("projected create role has no query token"))?;
        let members = create
            .roles()
            .get(role_id)
            .map_or_else(|| &[][..], Vec::as_slice);
        let wires = members
            .iter()
            .map(|value| projected_detached_reference_wire(package, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(wires, role.multiplicity())?,
        );
    }
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: None,
        value: None,
        values,
    })
}

fn canonical_struct_scalar(value: &AttributeValue) -> napi::Result<CanonicalValue> {
    match value {
        AttributeValue::String(value) => {
            type_bridge_contract::value::CanonicalString::new(value.clone())
                .map(CanonicalValue::String)
                .map_err(diagnostic_error)
        }
        AttributeValue::Long(value) => Ok(CanonicalValue::Long(*value)),
        AttributeValue::Double(value) => type_bridge_contract::value::CanonicalDouble::new(*value)
            .map(CanonicalValue::Double)
            .map_err(diagnostic_error),
        AttributeValue::Boolean(value) => Ok(CanonicalValue::Boolean(*value)),
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(CanonicalValue::Date)
            .map_err(diagnostic_error),
        AttributeValue::DateTime(value) => value
            .parse::<CanonicalDateTime>()
            .map(CanonicalValue::DateTime)
            .map_err(diagnostic_error),
        AttributeValue::DateTimeTZ(value) => {
            type_bridge_schema::parse_provider_datetime_tz_evidence(value)
                .map(CanonicalValue::DateTimeTz)
                .map_err(diagnostic_error)
        }
        AttributeValue::Decimal(value) => DecimalValue::new(value)
            .map(CanonicalValue::Decimal)
            .map_err(diagnostic_error),
        AttributeValue::Duration(value) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration)
            .map_err(diagnostic_error),
    }
}

fn projected_struct_wire(
    package: &InstalledPackage,
    value: &ProjectedStructValue,
) -> ProjectedStructWire {
    let projection = package
        .projection
        .projection()
        .structs()
        .values()
        .find(|projection| projection.id().label() == value.type_id().label())
        .expect("validated projected struct retains its installed projection");
    let values = projection
        .fields()
        .iter()
        .zip(value.members())
        .map(|(field, value)| {
            let value = value.as_ref().map(|value| {
                attribute_to_scalar(
                    &canonical_struct_attribute(value),
                    ValueType::from(field.value_type()),
                )
                .expect("validated projected struct scalar retains its declared domain")
            });
            (field.target_name().as_str().to_owned(), value)
        })
        .collect();
    ProjectedStructWire {
        type_key: canonical_type_key(value.type_id())
            .expect("validated projected struct identity is canonical"),
        values,
    }
}

fn canonical_struct_attribute(value: &CanonicalValue) -> AttributeValue {
    match value {
        CanonicalValue::String(value) => AttributeValue::String(value.as_str().to_owned()),
        CanonicalValue::Long(value) => AttributeValue::Long(*value),
        CanonicalValue::Double(value) => AttributeValue::Double(value.get()),
        CanonicalValue::Boolean(value) => AttributeValue::Boolean(*value),
        CanonicalValue::Date(value) => AttributeValue::Date(value.to_string()),
        CanonicalValue::DateTime(value) => AttributeValue::DateTime(value.to_string()),
        CanonicalValue::DateTimeTz(value) => AttributeValue::DateTimeTZ(value.to_string()),
        CanonicalValue::Decimal(value) => AttributeValue::Decimal(value.as_str().to_owned()),
        CanonicalValue::Duration(value) => AttributeValue::Duration(value.to_string()),
    }
}

fn projected_detached_reference_wire(
    package: &InstalledPackage,
    reference: &ProjectedReference,
) -> napi::Result<ProjectedWire> {
    Ok(ProjectedWire {
        type_key: canonical_type_key(reference.type_id())?,
        form: WireForm::Reference,
        iid: reference.iid().map(str::to_owned),
        value: None,
        values: projected_reference_fields_wire(package, reference)?,
    })
}

fn projected_thing_wire(
    package: &InstalledPackage,
    thing: &ProjectedThing,
) -> napi::Result<ProjectedWire> {
    thing
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let id = thing.type_id();
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("projected thing model is absent"))?;
    let mut values = projected_fields_wire(package, id, thing.fields(), false)?;
    if thing.roles().len() != model.complete_read().roles().len() {
        return Err(runtime_error(
            "projected thing roles do not match its complete-read projection",
        ));
    }
    for (role_id, read) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| runtime_error("projected read role has no query token"))?;
        let players = thing
            .roles()
            .get(role_id)
            .ok_or_else(|| runtime_error("projected thing omitted a selected role"))?
            .iter()
            .map(|player| projected_role_player_wire(package, player))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(players, read.multiplicity())?,
        );
    }
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: Some(thing.iid().to_owned()),
        value: None,
        values,
    })
}

fn projected_role_player_wire(
    package: &InstalledPackage,
    player: &ProjectedRolePlayer,
) -> napi::Result<ProjectedWire> {
    let form = match player.exact_form() {
        Some(ProjectedModelForm::Complete) => WireForm::Complete,
        Some(ProjectedModelForm::Reference) => WireForm::Reference,
        None => {
            return Err(runtime_error(
                "successor hydration requires exact role-player form evidence",
            ));
        }
    };
    let values = match form {
        WireForm::Complete => {
            projected_fields_wire(package, player.type_id(), player.fields(), false)?
        }
        WireForm::Reference => projected_reference_fields_wire(package, player.reference())?,
    };
    Ok(ProjectedWire {
        type_key: canonical_type_key(player.type_id())?,
        form,
        iid: Some(player.iid().to_owned()),
        value: None,
        values,
    })
}

fn projected_reference_fields_wire(
    package: &InstalledPackage,
    reference: &ProjectedReference,
) -> napi::Result<BTreeMap<String, Value>> {
    reference
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let model = package
        .projection
        .projection()
        .models()
        .get(reference.type_id())
        .ok_or_else(|| runtime_error("projected reference model is absent"))?;
    if reference.keys().len() != model.reference_read().key_fields().len() {
        return Err(runtime_error(
            "projected hydrated reference keys do not match its reference projection",
        ));
    }
    let mut values = BTreeMap::new();
    for field_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected reference key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == field_id)
            .ok_or_else(|| runtime_error("projected reference key has no read projection"))?;
        let value = reference
            .keys()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated reference omitted a key"))?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(
                vec![projected_attribute_value_wire(package, value)?],
                read.multiplicity(),
            )?,
        );
    }
    Ok(values)
}

fn projected_fields_wire(
    package: &InstalledPackage,
    id: &TypeId,
    fields: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    reference: bool,
) -> napi::Result<BTreeMap<String, Value>> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("projected hydrated model is absent"))?;
    let selected = model
        .complete_read()
        .fields()
        .iter()
        .filter(|field| !reference || model.reference_read().key_fields().contains(field.token()))
        .collect::<Vec<_>>();
    if fields.len() != selected.len() {
        return Err(runtime_error(
            "projected hydrated fields do not match the selected model form",
        ));
    }
    let mut values = BTreeMap::new();
    for read in selected {
        let field_id = read.token();
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated field has no query token"))?;
        let projected = fields
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated model omitted a selected field"))?;
        let hydrated = projected
            .iter()
            .map(|value| projected_attribute_value_wire(package, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(hydrated, read.multiplicity())?,
        );
    }
    Ok(values)
}

fn projected_attribute_value_wire(
    package: &InstalledPackage,
    value: &ProjectedAttributeValue,
) -> napi::Result<ProjectedWire> {
    value
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let model = package
        .projection
        .projection()
        .models()
        .get(value.attribute_type())
        .ok_or_else(|| runtime_error("projected attribute model is absent"))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| runtime_error("projected attribute has no scalar domain"))?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(value.attribute_type())?,
        form: WireForm::Complete,
        iid: None,
        value: Some(attribute_to_scalar(
            &value.to_attribute_value(),
            ValueType::from(value_type),
        )?),
        values: BTreeMap::new(),
    })
}

fn projected_read_member_value(
    values: Vec<ProjectedWire>,
    multiplicity: ProjectedMultiplicity,
) -> napi::Result<Value> {
    // `ProjectedThing`, `ProjectedCreate`, and `ProjectedReference` are
    // validated against the projection when built, cardinality included;
    // the multiplicity only picks the host shape here.
    match multiplicity.container() {
        ProjectedContainer::Scalar => values
            .into_iter()
            .next()
            .map(serde_json::to_value)
            .transpose()
            .map_err(json_error)
            .map(|value| value.unwrap_or(Value::Null)),
        ProjectedContainer::Sequence => serde_json::to_value(values).map_err(json_error),
    }
}

fn hydrate_entity(
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicEntityRow,
) -> napi::Result<ProjectedWire> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .entity_descriptor(id)
        .map_err(orm_error)?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: row.iid.clone(),
        value: None,
        values: hydrate_attributes(package, id, &descriptor.owned_attributes, &row.attributes)?,
    })
}

fn hydrate_relation(
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicRelationRow,
) -> napi::Result<ProjectedWire> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(orm_error)?;
    let mut values =
        hydrate_attributes(package, id, &descriptor.owned_attributes, &row.attributes)?;
    let projection = package.projection.projection();
    let model = projection
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("relation model is absent from its projection"))?;
    for read in model.complete_read().roles().values() {
        let token = model
            .query_tokens()
            .roles()
            .get(read.role())
            .ok_or_else(|| runtime_error("projected read role has no query token"))?;
        let role = descriptor
            .role(read.role().label().as_str())
            .ok_or_else(|| runtime_error("projected read role has no provider descriptor"))?;
        let players = row
            .role_players
            .iter()
            .filter(|player| player.role_name == read.role().label().as_str())
            .map(|player| hydrate_player(package, read.players(), player))
            .collect::<napi::Result<Vec<_>>>()?;
        let member = token.target_name().as_str();
        values.insert(
            member.to_owned(),
            projected_member_value(package, id, member, players, role_cardinality(role))?,
        );
    }
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: row.iid.clone(),
        value: None,
        values,
    })
}

fn hydrate_player(
    package: &InstalledPackage,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &DynamicRolePlayer,
) -> napi::Result<ProjectedWire> {
    let label = player
        .player_type_name
        .as_deref()
        .ok_or_else(|| runtime_error("role-player row has no concrete type label"))?;
    let id = package.type_by_label(label)?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| {
            runtime_error("role-player row type is not accepted by the projected role")
        })?;
    let attributes = package
        .projection
        .role_player_attributes(id, &player.attributes)
        .map_err(orm_error)?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            if id.kind() != TypeKind::Entity {
                return Err(runtime_error(
                    "nested complete relations are forbidden; project a reference",
                ));
            }
            hydrate_entity(
                package,
                id,
                &DynamicEntityRow {
                    iid: player.player_iid.clone(),
                    type_name: player.player_type_name.clone(),
                    attributes,
                },
            )
        }
        ProjectedModelForm::Reference => {
            let iid = player
                .player_iid
                .as_deref()
                .ok_or_else(|| runtime_error("reference role-player row has no IID"))?;
            ensure_iid(iid)?;
            let descriptors = match package.projection.descriptor(id).map_err(orm_error)? {
                TypeDescriptor::Entity(descriptor) => descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .cloned()
                    .collect::<Vec<_>>(),
                TypeDescriptor::Relation(_) => Vec::new(),
            };
            Ok(ProjectedWire {
                type_key: canonical_type_key(id)?,
                form: WireForm::Reference,
                iid: Some(iid.to_owned()),
                value: None,
                values: hydrate_attributes(package, id, &descriptors, &attributes)?,
            })
        }
    }
}

fn hydrate_attributes(
    package: &InstalledPackage,
    id: &TypeId,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &DynamicAttributeMap,
) -> napi::Result<BTreeMap<String, Value>> {
    let mut values = BTreeMap::new();
    for descriptor in descriptors {
        let wrappers = attributes
            .iter()
            .filter(|(name, _)| name == &descriptor.attr_name)
            .map(|(_, value)| attribute_wire(package, descriptor, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            descriptor.field_name.clone(),
            projected_member_value(
                package,
                id,
                &descriptor.field_name,
                wrappers,
                descriptor_cardinality(descriptor),
            )?,
        );
    }
    Ok(values)
}

fn attribute_wire(
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    value: &AttributeValue,
) -> napi::Result<ProjectedWire> {
    let id = package.type_by_label(&descriptor.attr_name)?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: None,
        value: Some(attribute_to_scalar(value, descriptor.value_type)?),
        values: BTreeMap::new(),
    })
}

fn projected_member_value(
    package: &InstalledPackage,
    id: &TypeId,
    member: &str,
    values: Vec<ProjectedWire>,
    cardinality: (u32, Option<u32>),
) -> napi::Result<Value> {
    validate_projected_hydrated_member_count(&package.projection, id, member, values.len())
        .map_err(napi_sdk_diagnostic)?;
    // The count is checked; the descriptor's maximum only picks the host shape.
    if cardinality.1 == Some(1) {
        values
            .into_iter()
            .next()
            .map(serde_json::to_value)
            .transpose()
            .map_err(json_error)
            .map(|value| value.unwrap_or(Value::Null))
    } else {
        serde_json::to_value(values).map_err(json_error)
    }
}

fn scalar_to_attribute(wire: &ScalarWire, expected: ValueType) -> napi::Result<AttributeValue> {
    if wire.value_type != value_type_tag(expected) {
        return Err(invalid_error(
            "scalar envelope value_type disagrees with the projection",
        ));
    }
    let text = || {
        wire.value
            .as_str()
            .ok_or_else(|| invalid_error("scalar envelope requires a string value"))
    };
    match expected {
        ValueType::String => text().map(|value| AttributeValue::String(value.to_owned())),
        ValueType::Long => {
            let value = text()?;
            let parsed = value
                .parse::<i64>()
                .map_err(|_| invalid_error("long envelope is outside i64"))?;
            if parsed.to_string() != value {
                return Err(invalid_error("long envelope is not canonical"));
            }
            Ok(AttributeValue::Long(parsed))
        }
        ValueType::Double => match wire.value.as_str() {
            Some("-0") => Ok(AttributeValue::Double(-0.0)),
            Some(_) => Err(invalid_error(
                "double envelope string must be canonical signed zero",
            )),
            None => wire
                .value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(AttributeValue::Double)
                .ok_or_else(|| invalid_error("double envelope requires a finite number")),
        },
        ValueType::Boolean => wire
            .value
            .as_bool()
            .map(AttributeValue::Boolean)
            .ok_or_else(|| invalid_error("boolean envelope requires a boolean")),
        ValueType::Date => canonical_temporal::<CanonicalDate>(text()?, AttributeValue::Date),
        ValueType::DateTime => {
            canonical_temporal::<CanonicalDateTime>(text()?, AttributeValue::DateTime)
        }
        ValueType::DateTimeTz => {
            canonical_temporal::<CanonicalDateTimeTz>(text()?, AttributeValue::DateTimeTZ)
        }
        ValueType::Decimal => {
            let value = text()?;
            let canonical = DecimalValue::new(value).map_err(diagnostic_error)?;
            if canonical.as_str() != value {
                return Err(invalid_error("decimal envelope is not canonical"));
            }
            Ok(AttributeValue::Decimal(value.to_owned()))
        }
        ValueType::Duration => {
            canonical_temporal::<CanonicalDuration>(text()?, AttributeValue::Duration)
        }
    }
}

fn scalar_to_ordered_attribute(
    wire: &ScalarWire,
    expected: ValueType,
) -> napi::Result<AttributeValue> {
    let timezone = match expected {
        ValueType::DateTime => false,
        ValueType::DateTimeTz => true,
        _ => return scalar_to_attribute(wire, expected),
    };
    let value = wire
        .value
        .as_str()
        .map(|value| Value::String(canonical_typescript_datetime(value, timezone)))
        .unwrap_or_else(|| wire.value.clone());
    scalar_to_attribute(
        &ScalarWire {
            value_type: wire.value_type,
            value,
        },
        expected,
    )
}

fn canonical_typescript_datetime(value: &str, timezone: bool) -> String {
    let local = if timezone {
        let Some(local) = value.strip_suffix('Z') else {
            return value.to_owned();
        };
        local
    } else {
        value
    };
    let Some((whole, fraction)) = local.rsplit_once('.') else {
        return value.to_owned();
    };
    if fraction.len() != 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_owned();
    }
    let fraction = fraction.trim_end_matches('0');
    let local = if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    };
    if timezone { format!("{local}Z") } else { local }
}

fn canonical_temporal<T>(
    value: &str,
    construct: impl FnOnce(String) -> AttributeValue,
) -> napi::Result<AttributeValue>
where
    T: FromStr<Err = type_bridge_contract::diagnostic::Diagnostic> + ToString,
{
    let canonical = value.parse::<T>().map_err(diagnostic_error)?;
    if canonical.to_string() != value {
        return Err(invalid_error("temporal envelope is not canonical"));
    }
    Ok(construct(value.to_owned()))
}

fn attribute_to_scalar(value: &AttributeValue, expected: ValueType) -> napi::Result<ScalarWire> {
    let (value_type, value) = match (value, expected) {
        (AttributeValue::String(value), ValueType::String) => {
            (ValueTypeTag::String, Value::String(value.clone()))
        }
        (AttributeValue::Long(value), ValueType::Long) => {
            (ValueTypeTag::Long, Value::String(value.to_string()))
        }
        (AttributeValue::Double(value), ValueType::Double) if value.is_finite() => {
            let value = if *value == 0.0 && value.is_sign_negative() {
                Value::String("-0".to_owned())
            } else {
                serde_json::Number::from_f64(*value)
                    .map(Value::Number)
                    .ok_or_else(|| runtime_error("provider returned a non-finite double"))?
            };
            (ValueTypeTag::Double, value)
        }
        (AttributeValue::Boolean(value), ValueType::Boolean) => {
            (ValueTypeTag::Boolean, Value::Bool(*value))
        }
        (AttributeValue::Date(value), ValueType::Date) => {
            (ValueTypeTag::Date, Value::String(value.clone()))
        }
        (AttributeValue::DateTime(value), ValueType::DateTime) => {
            (ValueTypeTag::DateTime, Value::String(value.clone()))
        }
        (AttributeValue::DateTimeTZ(value), ValueType::DateTimeTz) => {
            (ValueTypeTag::DateTimeTz, Value::String(value.clone()))
        }
        (AttributeValue::Decimal(value), ValueType::Decimal) => {
            (ValueTypeTag::Decimal, Value::String(value.clone()))
        }
        (AttributeValue::Duration(value), ValueType::Duration) => {
            (ValueTypeTag::Duration, Value::String(value.clone()))
        }
        _ => {
            return Err(runtime_error(
                "provider attribute value type disagrees with the projection",
            ));
        }
    };
    Ok(ScalarWire { value_type, value })
}

const fn value_type_tag(value: ValueType) -> ValueTypeTag {
    match value {
        ValueType::String => ValueTypeTag::String,
        ValueType::Long => ValueTypeTag::Long,
        ValueType::Double => ValueTypeTag::Double,
        ValueType::Boolean => ValueTypeTag::Boolean,
        ValueType::Date => ValueTypeTag::Date,
        ValueType::DateTime => ValueTypeTag::DateTime,
        ValueType::DateTimeTz => ValueTypeTag::DateTimeTz,
        ValueType::Decimal => ValueTypeTag::Decimal,
        ValueType::Duration => ValueTypeTag::Duration,
    }
}

fn descriptor_cardinality(descriptor: &OwnedAttributeDescriptor) -> (u32, Option<u32>) {
    descriptor
        .cardinality()
        .unwrap_or((u32::from(!descriptor.is_optional), Some(1)))
}

fn role_cardinality(descriptor: &RoleDescriptor) -> (u32, Option<u32>) {
    descriptor.cardinality.unwrap_or((0, Some(1)))
}

fn ensure_row_type(id: &TypeId, actual: Option<&str>) -> napi::Result<()> {
    if actual.is_some_and(|actual| actual != id.label().as_str()) {
        return Err(runtime_error(
            "exact provider row returned a different concrete type",
        ));
    }
    Ok(())
}

fn ensure_iid(value: &str) -> napi::Result<()> {
    if value.is_empty() {
        Err(invalid_error("IID must be a non-empty string"))
    } else {
        Ok(())
    }
}

fn wire_json(value: &ProjectedWire) -> napi::Result<String> {
    serde_json::to_string(value).map_err(json_error)
}

fn diagnostic_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    invalid_error(error.to_string())
}

fn node_canonical_limit_error(
    error: type_bridge_contract::diagnostic::Diagnostic,
    input: bool,
) -> Error {
    match SdkExecutionDiagnostic::from_canonical_limit(&error, input) {
        Some(diagnostic) => napi_sdk_diagnostic(diagnostic),
        None => diagnostic_error(error),
    }
}

fn node_canonical_codec_error(error: type_bridge_orm::ProjectedCodecError) -> Error {
    if let Some(diagnostic) = error.record_schema_mismatch() {
        return napi_sdk_diagnostic(diagnostic);
    }
    invalid_error(error.to_string())
}

fn orm_error(error: type_bridge_orm::OrmError) -> Error {
    runtime_error(error.to_string())
}

fn json_error(error: serde_json::Error) -> Error {
    runtime_error(error.to_string())
}

fn invalid_error(message: impl Into<String>) -> Error {
    Error::new(Status::InvalidArg, message.into())
}

fn runtime_error(message: impl Into<String>) -> Error {
    Error::new(Status::GenericFailure, message.into())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    #[cfg(feature = "contract-test-adapter")]
    use crate::contract_test_adapter::{
        ProjectionRecordingResponse, projection_recording_database_for_test,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{
        CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::temporal::TimeZoneDesignator;
    use type_bridge_contract::value::{CanonicalDouble, CanonicalString};
    use type_bridge_schema::{
        ManagedDeltaContext, SchemaDocumentSet, VerifiedSchemaAuthority, build_schema_authority,
        encode_schema_authority, normalize_documents, project,
    };
    use type_bridge_schema_codegen::{PythonEmitter, TypeScriptEmitter};

    use super::*;

    fn v3_root() -> PathBuf {
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

    fn v3_identity(root: &Path, relative: &str) -> Value {
        json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(fs::read(root.join(relative)).unwrap()))})
    }

    fn publish_models_node_fragment(results: Vec<Value>) {
        let destination = std::env::var_os("TYPE_BRIDGE_SDK_MODELS_PROOF_FRAGMENT");
        let nonce = std::env::var_os("TYPE_BRIDGE_SDK_MODELS_PROOF_RUN_NONCE");
        assert_eq!(destination.is_some(), nonce.is_some());
        let (Some(destination), Some(nonce)) = (destination, nonce) else {
            return;
        };
        let destination = PathBuf::from(destination);
        assert!(destination.is_absolute() && !destination.exists());
        let nonce = nonce.to_str().unwrap();
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let root = v3_root();
        let sources = [
            "type-bridge-core/crates/contract/src/sdk_diagnostic.rs",
            "type-bridge-core/crates/node/src/lib.rs",
            "type-bridge-core/crates/node/src/runtime_projection.rs",
            "type-bridge-core/crates/orm/src/execution_diagnostic.rs",
            "type-bridge-core/crates/orm/src/manager/dynamic.rs",
            "type-bridge-core/crates/orm/src/projected_crud.rs",
            "type-bridge-core/crates/orm/src/session/context.rs",
            "type-bridge-core/crates/orm/src/session/database.rs",
            "type-bridge-core/crates/orm/src/session/mod.rs",
            "type-bridge-core/crates/orm/src/session/transaction.rs",
            "type-bridge-core/crates/typedb-runtime/src/lib.rs",
        ];
        let fragment = json!({
            "binding": "node",
            "contract": {
                "allowlist": v3_identity(&root, "tests/contracts/sdk_conformance/models/proof-fragment-allowlist.json"),
                "journey": v3_identity(&root, "tests/contracts/sdk_conformance/models/journey.json"),
                "proof_schema": v3_identity(&root, "tests/contracts/sdk_conformance/models/proof-fragment-schema.json"),
            },
            "format": "typebridge.sdk-models-proof-fragment/v1",
            "producer": {"id": "node.sdk-models-generated-data-proof", "sources": sources.iter().map(|path| v3_identity(&root, path)).collect::<Vec<_>>()},
            "results": results,
            "run_nonce": nonce,
            "semantic_profile": crate::sdk_profile_matrix::requested("models").semantic_profile,
        });
        let mut bytes = to_canonical_json(&fragment).unwrap();
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
    fn sdk_models_node_data_plane_fragment() {
        use type_bridge_orm::{
            AnswerCancellation, DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES, DEFAULT_DIRECT_QUERY_BYTES,
            DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS, DEFAULT_DIRECT_QUERY_GRAPH_NODES,
            DEFAULT_DIRECT_QUERY_ITEMS, DEFAULT_DIRECT_QUERY_ROLE_PLAYERS,
            DEFAULT_DIRECT_QUERY_STATEMENTS, DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS,
            DirectConnectionPolicy, QueryExecutionResourceLimits,
        };
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
        let effective = QueryExecutionResourceLimits::tightened(
            DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS + 1,
            DEFAULT_DIRECT_QUERY_ITEMS + 1,
            DEFAULT_DIRECT_QUERY_BYTES + 1,
            DEFAULT_DIRECT_QUERY_GRAPH_NODES + 1,
            DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES + 1,
            DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS + 1,
            DEFAULT_DIRECT_QUERY_ROLE_PLAYERS + 1,
            DEFAULT_DIRECT_QUERY_STATEMENTS + 1,
        );
        assert_eq!(
            effective,
            QueryExecutionResourceLimits {
                statements: Some(DEFAULT_DIRECT_QUERY_STATEMENTS),
                ..Default::default()
            }
        );
        let policy = DirectConnectionPolicy::new("localhost:1729", "sdk", "admin", "secret")
            .connection_limits(effective)
            .answer_limits(effective);
        assert_eq!(format!("{policy:?}"), "DirectConnectionPolicy([REDACTED])");
        // Historical-policy replay: 07.0 changed the default timeout from 30 s to 60 s.
        assert_eq!(
            QueryExecutionResourceLimits::default_for(type_bridge_orm::AnswerTransport::Proxy)
                .timeout_milliseconds
                .milliseconds(),
            Some(60_000),
        );
        let wire_limits = QueryExecutionResourceLimits {
            collect_phase_timestamps: false,
            timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(30_000),
            items: 65_536,
            bytes: 33_554_432,
            graph_nodes: 65_536,
            attribute_values: 65_536,
            collection_members: 65_536,
            role_players: 65_536,
            statements: Some(3),
        }
        .validate()
        .expect("historical policy is accepted");
        assert_eq!(effective.constrained_by(wire_limits), wire_limits);
        let dimensions = [
            (
                "timeout_milliseconds",
                wire_limits.timeout_milliseconds.milliseconds().unwrap_or(0),
            ),
            ("items", wire_limits.items),
            ("bytes", wire_limits.bytes),
            ("graph_nodes", wire_limits.graph_nodes),
            ("attribute_values", wire_limits.attribute_values),
            ("collection_members", wire_limits.collection_members),
            ("role_players", wire_limits.role_players),
            ("statements", u64::from(wire_limits.statement_limit())),
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
        let cancelled = json!({
            "pre_dispatch": {"category": "cancelled", "code": "provider_cancelled", "provider_calls": 0, "partial_result": false},
            "owned_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "rollback_completed": true, "committed_prefix": false, "published_results": 0},
            "borrowed_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "state": "rollback_only", "first_cause_retained": true, "commit_rejected": "transaction_rollback_only", "rollback_available": true},
            "after_commit": {"committed_outcome_preserved": true, "relabeled_cancelled": false}
        });
        let limits = json!({
            "dimensions": dimensions, "absolute_deadline_reused": true, "connection_ceiling_inherited": true,
            "operation_policy_tightens_only": true,
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
            }, "provider_text_exposed": false, "secrets_exposed": false, "deterministic_field_order": true
        });
        let id = "runtime_projection::tests::sdk_models_node_data_plane_fragment";
        publish_models_node_fragment(vec![
            json!({"observation": connection, "observation_ref": "complete_connection_policy", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": id}),
            json!({"observation": cancelled, "observation_ref": "data_operation_cancellation", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": id}),
            json!({"observation": limits, "observation_ref": "data_operation_resource_limits", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": id}),
            json!({"observation": diagnostic, "observation_ref": "data_operation_structured_diagnostic", "outcome": "passed", "proof_kind": "diagnostic", "test_id": id}),
        ]);
    }

    #[test]
    fn direct_tls_shape_rejects_before_provider_runtime_creation() {
        for (mode, root, message) in [
            ("custom_root", None, "custom_root TLS requires tlsRootCa"),
            (
                "disabled",
                Some("unused.pem".to_owned()),
                "tlsRootCa is valid only with custom_root TLS",
            ),
            (
                "invented",
                None,
                "tlsMode must be disabled, native_roots, or custom_root",
            ),
        ] {
            let error = direct_tls_from_node(mode, root).expect_err("TLS shape must fail");
            assert!(error.to_string().contains(message));
        }
    }

    const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag:
    value:
      type: string
      regex: "^[a-z]+$"
  score: { value: integer }
  val-double: { value: double }
  val-datetime: { value: datetime }
  val-datetime-tz: { value: datetime-tz }
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
    owns:
      identifier: { key: true }
      score: { card: 1, range: { min: 1, max: 5 } }
  group:
    sub: actor
relations:
  membership:
    relates:
      member:
        card: { min: 1, max: 4 }
        ordered: true
        distinct: true
  base-activity:
    relates:
      participant: { abstract: true, card: 1 }
  plain-activity:
    sub: base-activity
  activity-link:
    relates:
      subject:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
plays:
  person:
    membership:
      member: { card: { min: 0, max: 4 } }
    base-activity:
      participant: { card: { min: 0, max: 1 } }
  group:
    base-activity:
      participant: { card: { min: 0, max: 1 } }
  plain-activity:
    activity-link:
      subject: { card: { min: 0, max: 4 } }
"#;

    const UNORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  score: { value: integer }
entities:
  person:
    owns:
      score: { card: 1, range: { min: 1, max: 5 } }
"#;

    fn authority(source: &str, document: &str) -> VerifiedSchemaAuthority {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new(document).unwrap(), source)]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("node-native").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            schema_authority_capability_vocabulary(),
        );
        build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap()
    }

    fn typescript_projection(authority: &VerifiedSchemaAuthority) -> RuntimeProjection {
        let emitter = TypeScriptEmitter::new();
        project(
            authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap()
    }

    fn evidence(projection: &RuntimeProjection) -> (String, String, String) {
        (
            String::from_utf8(to_canonical_json(projection).unwrap()).unwrap(),
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap(),
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap(),
        )
    }

    fn registrations(projection: &RuntimeProjection) -> String {
        serde_json::to_string(
            &projection
                .models()
                .iter()
                .map(|(id, model)| {
                    serde_json::json!({
                        "typeKey": canonical_type_key(id).unwrap(),
                        "targetName": model.target_name().as_str(),
                        "create": model.create().enabled(),
                        "reference": model.reference_read().target_name().is_some(),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn assert_projection_evidence_mismatch(error: Error) {
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(
            diagnostic,
            serde_json::json!({
                "version": 1,
                "category": "integrity",
                "sdk_category": "integrity",
                "query_category": null,
                "code": "projection_evidence_mismatch",
                "message": "Generated projection evidence does not match the verified schema package",
                "path": [{"kind": "argument", "value": "projection_evidence"}],
                "details": {},
            })
        );
    }

    fn assert_missing_semantic_fingerprint(error: Error) {
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(
            diagnostic,
            serde_json::json!({
                "version": 1,
                "category": "integrity",
                "sdk_category": "integrity",
                "query_category": null,
                "code": "projection_evidence_mismatch",
                "message": "Generated projection evidence does not match the verified schema package",
                "path": [
                    {"kind": "argument", "value": "projection_evidence"},
                    {"kind": "index", "value": 0},
                    {
                        "kind": "contract_identity",
                        "value": "semantic_schema_fingerprint",
                    },
                ],
                "details": {
                    "actual_occurrence_count": {"kind": "count", "value": "0"},
                    "expected_occurrence_count": {"kind": "count", "value": "1"},
                    "foreign_package": {"kind": "boolean", "value": false},
                },
            })
        );
    }

    fn runtime_for_schema(source: &str, document: &str) -> NodeRuntimeProjection {
        let authority = authority(source, document);
        let authority_json = String::from_utf8(encode_schema_authority(&authority)).unwrap();
        let projection = typescript_projection(&authority);
        let registrations = registrations(&projection);
        let (projection, semantic, fingerprint) = evidence(&projection);
        NodeRuntimeProjection::install(
            projection,
            semantic,
            fingerprint,
            registrations,
            Some(authority_json),
            None,
            false,
        )
        .unwrap()
    }

    fn ordered_runtime() -> NodeRuntimeProjection {
        runtime_for_schema(ORDERED_SCHEMA, "node-create-admission.yaml")
    }

    fn manager_without_execution_target(
        runtime: &NodeRuntimeProjection,
        type_id: TypeId,
    ) -> NodeProjectedModelManager {
        NodeProjectedModelManager {
            package: Arc::clone(&runtime.package),
            type_id,
            target: None,
            successor_batch_marker: None,
            runtime: Arc::new(
                ProviderRuntimeOwner::new().expect("provider runtime should start for native test"),
            ),
            filters: vec![],
            closed_filter: None,
            has_empty_iids: false,
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    fn one_row_insert_batch(
        runtime: &NodeRuntimeProjection,
        type_id: TypeId,
        wire: ProjectedWire,
    ) -> (ProjectedBatch, ProjectedBatchInvocationControl) {
        let control = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        );
        let create = project_create_wire(runtime.package.as_ref(), &type_id, &wire).unwrap();
        let batch = ProjectedBatch::try_new_for_invocation(
            runtime.package.projection.as_ref(),
            type_id,
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(create)],
            &control,
        )
        .unwrap();
        (batch, control)
    }

    #[test]
    fn proxy_batch_target_validates_empty_calls_and_reaches_the_staged_transport() {
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        use type_bridge_orm::plan_authority::QueryAuthority;
        use type_bridge_orm::remote_signing::RemoteReplySigningKey;
        let verified = authority(ORDERED_SCHEMA, "node-proxy-batch.yaml");
        let declared =
            type_bridge_contract::schema::encode_declared_schema(verified.declared_schema())
                .unwrap();
        let authority =
            QueryAuthority::from_declared_bytes(&declared, "node-native", "typedb-3.12.1/v1")
                .unwrap();
        let advertisement = RemoteCapabilities::new(
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary(),
            RemoteExecutorBinding::new("batch-target-selector", "epoch-00000000001").unwrap(),
            RemoteReplySigningKey::from_secret_bytes([0x5c; 32]).public_key(),
        )
        .encode()
        .unwrap();
        let proxy = type_bridge_orm::ProxyQueryConnection::with_exchange(
            advertisement,
            Arc::new(authority),
            QueryExecutionResourceLimits::default(),
        )
        .unwrap();
        let runtime = ordered_runtime();
        let model = TypeId::new(TypeKind::Entity, "person").unwrap();
        let mut manager = manager_without_execution_target(&runtime, model.clone());
        manager.target = Some(OwnedQueryTarget::Proxy(Arc::new(proxy)));
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
            ProjectedBatchOperation::Delete,
        ] {
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let batch = ProjectedBatch::try_new_for_invocation(
                manager.package.projection.as_ref(),
                model.clone(),
                operation,
                vec![],
                &control,
            )
            .unwrap();
            assert_eq!(
                manager
                    .execute_projected_batch(&batch, control, false, 7, |_| panic!(
                        "empty mapper invoked"
                    ),)
                    .unwrap(),
                7
            );
        }
        for returning in [false, true] {
            let (batch, control) = manager
                .prepare_returning_delete(vec!["0xa1".into()])
                .unwrap();
            let error = manager
                .execute_projected_batch(&batch, control, returning, (), |_| {
                    panic!("no transport installed")
                })
                .unwrap_err();
            assert_eq!(error.code().as_str(), "proxy_exchange_caller_owned");
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn keyword_reads_keep_native_origins_for_direct_and_borrowed_transactions() {
        use type_bridge_orm::TxType;
        use type_bridge_orm::session::backend::QueryResult;
        for borrowed in [None, Some(TxType::Read), Some(TxType::Write)] {
            let projection = ordered_runtime();
            let doc = serde_json::json!({"_iid":"0xa1","_type":"person","attributes":{
                "identifier":"person-1","score":3,"tag":[]
            }});
            let (database, state) = projection_recording_database_for_test(vec![
                ProjectionRecordingResponse::Query(QueryResult::Documents(vec![doc.clone()])),
                ProjectionRecordingResponse::Query(QueryResult::Documents(vec![doc])),
                ProjectionRecordingResponse::Query(QueryResult::Rows(vec![
                    serde_json::json!({"count2":{"kind":"value","value":type_bridge_contract::value::CanonicalValue::Long(1)}}),
                ])),
                ProjectionRecordingResponse::Query(QueryResult::Rows(vec![
                    serde_json::json!({"root0":{"kind":"thing","iid":"0xa1","type_id":{"kind":"entity","label":"person"}}}),
                ])),
                ProjectionRecordingResponse::Query(QueryResult::Documents(vec![])),
            ]);
            let runtime = Arc::new(ProviderRuntimeOwner::new().unwrap());
            let transaction = borrowed.map(|kind| {
                runtime
                    .block_on(database.transaction_context(kind))
                    .unwrap()
            });
            let manager = NodeProjectedModelManager {
                package: Arc::clone(&projection.package),
                type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
                target: Some(transaction.as_ref().map_or_else(
                    || OwnedQueryTarget::Database(Arc::clone(&database)),
                    |tx| OwnedQueryTarget::Transaction(tx.clone()),
                )),
                successor_batch_marker: None,
                runtime: Arc::clone(&runtime),
                filters: vec![],
                closed_filter: None,
                has_empty_iids: false,
            };
            let manager = manager.filter_entries_json(serde_json::json!([{
                "name":"identifier__in", "value":[attribute_wire("identifier", ValueTypeTag::String, Value::String("person-1".into()))]
            }]).to_string()).unwrap();
            let all = manager.all_projected().unwrap();
            assert_eq!(all.len(), 1);
            let first = manager.first_projected().unwrap().unwrap();
            for envelope in [&all[0], &first] {
                assert_eq!(envelope.thing.iid(), "0xa1");
                assert!(
                    envelope
                        .thing
                        .try_to_reference(projection.package.projection.as_ref())
                        .unwrap()
                        .origin_carrier()
                        .is_some()
                );
            }
            assert_eq!(manager.count().unwrap().words, [1]);
            assert!(manager.exists().unwrap());
            assert!(manager.first_projected().unwrap().is_none());
            if let Some(tx) = transaction {
                assert_eq!(
                    runtime.block_on(tx.lifecycle_state()),
                    TransactionContextState::Active
                );
            }
            let state = state.lock().unwrap();
            assert_eq!(state.queries.len(), 5);
            assert_eq!((state.commits, state.rollbacks), (0, 0));
        }
    }

    fn type_key(kind: TypeKind, label: &str) -> String {
        canonical_type_key(&TypeId::new(kind, label).unwrap()).unwrap()
    }

    fn attribute_wire(label: &str, value_type: ValueTypeTag, value: Value) -> ProjectedWire {
        ProjectedWire {
            type_key: type_key(TypeKind::Attribute, label),
            form: WireForm::Complete,
            iid: None,
            value: Some(ScalarWire { value_type, value }),
            values: BTreeMap::new(),
        }
    }

    #[test]
    fn scalar_envelopes_preserve_long_and_reject_noncanonical_domains() {
        let long = ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9007199254740993".into()),
        };
        assert_eq!(
            scalar_to_attribute(&long, ValueType::Long).unwrap(),
            AttributeValue::Long(9_007_199_254_740_993)
        );
        let leading_zero = ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("01".into()),
        };
        assert!(scalar_to_attribute(&leading_zero, ValueType::Long).is_err());
        let date = ScalarWire {
            value_type: ValueTypeTag::Date,
            value: Value::String("2024-02-29".into()),
        };
        assert_eq!(
            scalar_to_attribute(&date, ValueType::Date).unwrap(),
            AttributeValue::Date("2024-02-29".into())
        );
        let bad_date = ScalarWire {
            value_type: ValueTypeTag::Date,
            value: Value::String("2023-02-29".into()),
        };
        assert!(scalar_to_attribute(&bad_date, ValueType::Date).is_err());
    }

    #[test]
    fn canonical_manager_filter_is_immutable_and_rejects_token_mismatches_before_io() {
        let runtime = ordered_runtime();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let manager = manager_without_execution_target(&runtime, person.clone());
        let root = manager.manager_filter().unwrap();
        assert!(root.filter.is_empty());

        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("alice".into()),
        );
        let filtered = root
            .and_projected(
                type_key(TypeKind::Entity, "person"),
                "\"identifier\"".into(),
                "eq".into(),
                wire_json(&identifier).unwrap(),
            )
            .unwrap();
        assert!(root.filter.is_empty());
        assert_eq!(filtered.filter.len(), 1);

        let wrong_owner = root
            .and_projected(
                type_key(TypeKind::Relation, "membership"),
                "\"identifier\"".into(),
                "eq".into(),
                wire_json(&identifier).unwrap(),
            )
            .err()
            .expect("wrong-owner field must reject");
        let diagnostic: Value = serde_json::from_str(&wrong_owner.reason).unwrap();
        assert_eq!(diagnostic["category"], "integrity");
        assert_eq!(diagnostic["code"], "field_owner_mismatch");

        let wrong_domain = root
            .and_projected(
                type_key(TypeKind::Entity, "person"),
                "\"score\"".into(),
                "eq".into(),
                wire_json(&identifier).unwrap(),
            )
            .err()
            .expect("wrong-domain value must reject");
        let diagnostic: Value = serde_json::from_str(&wrong_domain.reason).unwrap();
        assert_eq!(diagnostic["category"], "invalid_input");
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");

        let foreign = root.reject_foreign_field_token().unwrap_err();
        let diagnostic: Value = serde_json::from_str(&foreign.reason).unwrap();
        assert_eq!(diagnostic["category"], "integrity");
        assert_eq!(diagnostic["code"], "generated_token_package_mismatch");
    }

    #[test]
    fn keyword_selection_is_retained_by_canonical_filters() {
        let runtime = ordered_runtime();
        let manager = manager_without_execution_target(
            &runtime,
            TypeId::new(TypeKind::Entity, "person").unwrap(),
        );
        let filtered = manager.filter_entries_json(serde_json::json!([
            {"name":"identifier", "value": attribute_wire("identifier", ValueTypeTag::String, Value::String("alice".into()))},
            {"name":"iid", "value":"0xa1"}
        ]).to_string()).unwrap();
        let canonical = filtered.manager_filter().unwrap();
        assert_eq!(canonical.filter.len(), 2);
        assert!(manager.manager_filter().unwrap().filter.is_empty());
        let chained = filtered.filter_entries_json(serde_json::json!([
            {"name":"score__gte", "value": attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()))}
        ]).to_string()).unwrap();
        assert_eq!(chained.manager_filter().unwrap().filter.len(), 3);
        let wider = filtered.filter_entries_json(serde_json::json!([
            {"name":"identifier__contains", "value": attribute_wire("identifier", ValueTypeTag::String, Value::String("a".into()))}
        ]).to_string()).unwrap();
        assert!(wider.closed_filter.is_none());
        let error = wider
            .manager_filter()
            .err()
            .expect("wider selection must not be discarded");
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "unsupported_manager_filter");
        assert_eq!(wider.filters.len(), 3);
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn empty_keyword_iid_reads_send_no_requests_even_with_wider_lookups() {
        let projection = ordered_runtime();
        let (database, state) = projection_recording_database_for_test(vec![]);
        let mut manager = manager_without_execution_target(
            &projection,
            TypeId::new(TypeKind::Entity, "person").unwrap(),
        );
        manager.target = Some(OwnedQueryTarget::Database(database));
        let empty = manager.filter_entries_json(serde_json::json!([
            {"name":"iid__in", "value":[]},
            {"name":"identifier__contains", "value": attribute_wire("identifier", ValueTypeTag::String, Value::String("a".into()))}
        ]).to_string()).unwrap();
        assert!(empty.all_projected().unwrap().is_empty());
        assert!(empty.first_projected().unwrap().is_none());
        assert_eq!(empty.count().unwrap().words, [0]);
        assert!(!empty.exists().unwrap());
        assert!(empty.manager_filter().is_err());
        manager
            .filter_entries_json(
                serde_json::json!([
                    {"name":"iid__in", "value":[]}
                ])
                .to_string(),
            )
            .unwrap()
            .manager_filter()
            .unwrap()
            .delete()
            .unwrap();
        let large = manager.filter_entries_json(serde_json::json!([
            {"name":"iid__in", "value": (0..257).map(|index| format!("0x{index:x}")).collect::<Vec<_>>()}
        ]).to_string()).unwrap();
        for result in [
            large.all_projected().map(|_| ()),
            large.first_projected().map(|_| ()),
            large.count().map(|_| ()),
            large.exists().map(|_| ()),
        ] {
            assert!(result.is_err());
        }
        assert!(state.lock().unwrap().queries.is_empty());
    }

    #[test]
    fn projected_batch_authority_publishes_only_finished_rows() {
        let projection = Arc::clone(&ordered_runtime().package.projection);

        let aborted = NodeProjectedBatchAuthority::pending(Arc::clone(&projection));
        {
            let _guard = PendingBatchAuthority::new(Arc::clone(&aborted));
            assert!(!aborted.is_active());
        }
        assert_eq!(
            aborted.state.load(Ordering::Acquire),
            BATCH_AUTHORITY_ABORTED
        );
        assert!(!aborted.is_active());

        let active = NodeProjectedBatchAuthority::pending(projection);
        active.rows.set(Vec::new()).unwrap();
        PendingBatchAuthority::new(Arc::clone(&active)).finish();
        assert_eq!(active.state.load(Ordering::Acquire), BATCH_AUTHORITY_ACTIVE);
        assert!(active.is_active());
        active.abort();
        assert!(active.is_active(), "an active authority cannot be revoked");
    }

    #[test]
    fn projected_batch_role_selector_retains_only_fixed_size_indices() {
        assert!(
            std::mem::size_of::<NodeProjectedBatchProofSelector>()
                <= 3 * std::mem::size_of::<u32>(),
            "a batch proof selector must not retain caller-sized role text"
        );
        let selector = NodeProjectedBatchProofSelector::Role {
            role_ordinal: 65_535,
            player_index: 65_534,
        };
        assert!(matches!(
            selector,
            NodeProjectedBatchProofSelector::Role {
                role_ordinal: 65_535,
                player_index: 65_534,
            }
        ));
    }

    #[test]
    fn projected_batch_role_index_handles_role_last_repeated_lookup() {
        const ROLE_COUNT: u32 = 4_096;
        let mut role_ids = Vec::with_capacity(ROLE_COUNT as usize);
        let mut role_ordinals_by_target = BTreeMap::new();
        for ordinal in 0..ROLE_COUNT {
            let target = format!("role-{ordinal:04}");
            role_ids.push(RoleId::new("membership", "member").unwrap());
            role_ordinals_by_target.insert(target, ordinal);
        }
        let index = ProjectedRoleIndex {
            role_ids,
            role_ordinals_by_target,
        };
        let last_target = format!("role-{:04}", ROLE_COUNT - 1);
        for _ in 0..MAX_CANONICAL_COLLECTION_LEN {
            let ordinal = index.role_ordinal(&last_target).unwrap();
            assert_eq!(ordinal, ROLE_COUNT - 1);
            assert_eq!(index.role_id(ordinal).unwrap().label().as_str(), "member");
        }
    }

    #[test]
    fn projected_batch_ingress_reservation_failure_preserves_common_diagnostic() {
        let shape = || {
            batch_input_shape_diagnostic(
                "generated_model_layout_mismatch",
                "The generated batch row no longer has its installed runtime shape",
                &projected_batch_row_path(0),
            )
        };
        FAIL_PROJECTED_BATCH_INGRESS_RESERVATION.store(true, Ordering::Relaxed);
        let string_diagnostic = match reserved_projected_binding_bytes(8) {
            Ok(_) => panic!("the deterministic string reservation seam must fail"),
            Err(diagnostic) => diagnostic,
        };
        FAIL_PROJECTED_BATCH_INGRESS_RESERVATION.store(true, Ordering::Relaxed);
        let proof_diagnostic = match reserved_projected_proof_slots(8) {
            Ok(_) => panic!("the deterministic proof reservation seam must fail"),
            Err(diagnostic) => diagnostic,
        };
        for diagnostic in [string_diagnostic, proof_diagnostic] {
            let error = raw_projected_binding_or_shape(
                RawProjectedBindingError::Diagnostic(diagnostic),
                shape,
            );
            let rendered: Value = serde_json::from_str(&error.reason).unwrap();
            assert_eq!(rendered["code"], "projected_batch_allocation_exhausted");
        }
    }

    #[test]
    fn projected_batch_role_selector_rejects_wrong_role_and_player_indices() {
        let runtime = ordered_runtime();
        let package = runtime.package.as_ref();
        let link_id = TypeId::new(TypeKind::Relation, "activity-link").unwrap();
        let player_id = TypeId::new(TypeKind::Relation, "plain-activity").unwrap();
        let link_model = package
            .projection
            .projection()
            .models()
            .get(&link_id)
            .unwrap();
        let (role_id, read_role) = link_model.complete_read().roles().iter().next().unwrap();
        let reference = ProjectedReference::try_new_for_hydration(
            package.projection.as_ref(),
            player_id,
            Some("0xd1".into()),
            Vec::new(),
        )
        .unwrap();
        let player = ProjectedRolePlayer::try_new_reference_for_hydration(
            package.projection.as_ref(),
            read_role,
            reference,
        )
        .unwrap();
        let relation = ProjectedThing::try_new(
            package.projection.as_ref(),
            link_id,
            "0xe1".into(),
            Vec::new(),
            vec![(role_id.clone(), vec![player])],
        )
        .unwrap();
        let expected = relation.roles()[role_id][0].reference().clone();
        let authority = NodeProjectedBatchAuthority::pending(Arc::clone(&package.projection));
        authority.rows.set(vec![relation]).unwrap();
        PendingBatchAuthority::new(Arc::clone(&authority)).finish();

        let proof = |role_ordinal, player_index| {
            NodeProjectedInputProof::Batch(NodeProjectedBatchProof {
                authority: Arc::clone(&authority),
                row: 0,
                selector: NodeProjectedBatchProofSelector::Role {
                    role_ordinal,
                    player_index,
                },
            })
        };
        assert_eq!(proof(0, 0).reference(package).unwrap(), expected);
        for rejected in [proof(1, 0), proof(0, 1)] {
            assert_eq!(
                rejected.reference(package).unwrap_err().code().as_str(),
                "malformed_projected_create"
            );
        }
    }

    #[test]
    fn projected_batch_shape_diagnostics_retain_exact_row_paths() {
        let row = batch_input_shape_diagnostic(
            "generated_model_layout_mismatch",
            "The generated batch row no longer has its installed runtime shape",
            &projected_batch_row_path(2),
        );
        assert_eq!(row.code().as_str(), "generated_model_layout_mismatch");
        assert!(matches!(
            row.path(),
            [SdkDiagnosticPathSegment::Argument(argument), SdkDiagnosticPathSegment::Index(2)]
                if argument.as_str() == "rows"
        ));

        let iid = batch_input_shape_diagnostic(
            "batch_iid_type_mismatch",
            "Successor update rows require exact string IIDs",
            &projected_batch_iid_path(1),
        );
        assert_eq!(iid.code().as_str(), "batch_iid_type_mismatch");
        assert!(matches!(
            iid.path(),
            [
                SdkDiagnosticPathSegment::Argument(rows),
                SdkDiagnosticPathSegment::Index(1),
                SdkDiagnosticPathSegment::Argument(iid),
            ] if rows.as_str() == "rows" && iid.as_str() == "iid"
        ));
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn authored_plan_mixed_rows_preserve_core_ordinals_canonical_scalars_and_origins() {
        use type_bridge_contract::query_plan::v3::{BuiltPlans, PlanBuilder};
        use type_bridge_orm::session::backend::QueryResult;
        let projection = ordered_runtime();
        let compiled = authority(ORDERED_SCHEMA, "node-create-admission.yaml");
        let mut builder = PlanBuilder::new();
        let group_id = TypeId::new(TypeKind::Entity, "group").unwrap();
        let group = builder.exact(group_id.clone());
        let tag = builder.field(
            group,
            type_bridge_contract::id::AttributeId::new("tag").unwrap(),
        );
        builder.returning(vec![group, tag]);
        let plan = builder
            .build(
                compiled.declared_schema(),
                compiled.managed_state().managed_semantic_schema().clone(),
            )
            .unwrap();
        let row = serde_json::Value::Object(
            [
                (
                    plan.bindings()[usize::from(group.get())]
                        .variable()
                        .as_str()
                        .to_owned(),
                    serde_json::to_value(type_bridge_contract::query_remote::RemoteValue::Thing {
                        iid: "0xa1".into(),
                        type_id: group_id,
                    })
                    .unwrap(),
                ),
                (
                    plan.bindings()[usize::from(tag.get())]
                        .variable()
                        .as_str()
                        .to_owned(),
                    serde_json::to_value(type_bridge_contract::query_remote::RemoteValue::Value {
                        value: CanonicalValue::String(CanonicalString::new("one").unwrap()),
                    })
                    .unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let model = projection
            .prepare_authored_tuple(
                BuiltPlans::single(plan),
                vec![u32::from(group.get())],
                vec![type_key(TypeKind::Entity, "group")],
            )
            .unwrap();
        let responses = model.stack.plans().iter().map(|plan| ProjectionRecordingResponse::Query(if plan.fetch_document_shape().is_some() {
            QueryResult::Documents(vec![serde_json::json!({"_iid":"0xa1", "_type":"group", "attributes":{"tag":["one"]}})])
        } else { QueryResult::Rows(vec![row.clone(), row.clone()]) })).collect();
        let registry = Arc::clone(&model.registry);
        let source = Arc::new(crate::plan_builder_runtime::NodePlanProgram::Models(
            Box::new(model),
        ));
        let (database, _) = projection_recording_database_for_test(responses);
        let runtime = ProviderRuntimeOwner::new().unwrap();
        let mut transaction = database.transaction(Default::default());
        let pending = transaction
            .stage_plan_fetch(Arc::clone(&source), registry, Default::default())
            .unwrap();
        runtime.block_on(transaction.commit()).unwrap();
        let proof = pending.get().unwrap();
        assert_eq!(proof.model_rows(), &[0, 0]);
        let crate::plan_builder_runtime::NodePlanProgram::Models(model) = source.as_ref() else {
            unreachable!()
        };
        let rows = model
            .materialize_rows(
                &proof,
                type_bridge_orm::ProjectedQueryOrigin::for_database(&database),
                &OnceLock::new(),
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            let model = row[0].model.as_ref().unwrap();
            assert_eq!(model.thing.iid(), "0xa1");
            assert!(model.thing.origin_carrier().is_some());
            let scalar: CanonicalValue =
                serde_json::from_str(row[1].scalar.as_ref().unwrap()).unwrap();
            assert!(matches!(scalar, CanonicalValue::String(value) if value.as_str() == "one"));
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn authored_plan_model_envelopes_consume_core_proofs_and_keep_direct_deleted_origins() {
        use type_bridge_contract::query_plan::v3::{BuiltPlans, PlanBuilder};
        use type_bridge_orm::session::backend::QueryResult;
        for deleted in [false, true] {
            let projection = ordered_runtime();
            let compiled = authority(ORDERED_SCHEMA, "node-create-admission.yaml");
            let mut builder = PlanBuilder::new();
            let group = builder.exact(TypeId::new(TypeKind::Entity, "group").unwrap());
            builder.fetch_held(group, vec![]);
            if deleted {
                builder.delete(group);
            }
            let plan = builder
                .build(
                    compiled.declared_schema(),
                    compiled.managed_state().managed_semantic_schema().clone(),
                )
                .unwrap();
            let model = projection
                .prepare_authored_plan(
                    BuiltPlans::single(plan),
                    group,
                    &type_key(TypeKind::Entity, "group"),
                )
                .unwrap();
            let registry = Arc::clone(&model.registry);
            let source = Arc::new(crate::plan_builder_runtime::NodePlanProgram::Models(
                Box::new(model),
            ));
            let mut responses = vec![ProjectionRecordingResponse::Query(QueryResult::Documents(
                vec![serde_json::json!({"_iid":"0xa1", "_type":"group", "attributes":{"tag":[]}})],
            ))];
            if deleted {
                responses.push(ProjectionRecordingResponse::Query(QueryResult::Rows(vec![
                    serde_json::json!({}),
                ])));
            }
            let (database, _) = projection_recording_database_for_test(responses);
            let runtime = ProviderRuntimeOwner::new().unwrap();
            let mut transaction = database.transaction(Default::default());
            let pending = transaction
                .stage_plan_fetch(Arc::clone(&source), registry, Default::default())
                .unwrap();
            runtime.block_on(transaction.commit()).unwrap();
            let proof = pending.get().unwrap();
            let crate::plan_builder_runtime::NodePlanProgram::Models(model) = source.as_ref()
            else {
                unreachable!()
            };
            let cache = OnceLock::new();
            let values = model
                .materialize(
                    &proof,
                    type_bridge_orm::ProjectedQueryOrigin::for_database(&database),
                    &cache,
                )
                .unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].thing.iid(), "0xa1");
            assert_eq!(values[0].deleted(), deleted);
            assert!(values[0].thing.origin_carrier().is_some());
            if deleted {
                assert_eq!(
                    values[0]
                        .thing
                        .try_to_reference(&projection.package.projection)
                        .err()
                        .unwrap()
                        .code()
                        .as_str(),
                    "thing_deleted"
                );
            } else {
                assert!(
                    values[0]
                        .thing
                        .try_to_reference(&projection.package.projection)
                        .unwrap()
                        .origin_carrier()
                        .is_some()
                );
            }
            let original = std::ptr::from_ref(cache.get().unwrap());
            model
                .materialize(
                    &proof,
                    type_bridge_orm::ProjectedQueryOrigin::for_database(&database),
                    &cache,
                )
                .unwrap();
            assert_eq!(original, std::ptr::from_ref(cache.get().unwrap()));
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn node_single_projected_writes_materialize_complete_results_before_completion() {
        use type_bridge_orm::TxType;
        use type_bridge_orm::session::backend::QueryResult;

        let schema = ORDERED_SCHEMA.replace(
            "      subject:\n        card: { min: 0, max: 4 }\n        ordered: true\n        distinct: true",
            "      subject:\n        card: { min: 0, max: 4 }",
        );
        for schema in [
            schema.clone(),
            schema
                .replace("        ordered: true\n", "")
                .replace("        distinct: true\n", ""),
        ] {
            for (kind, label) in [
                (TypeKind::Entity, "group"),
                (TypeKind::Relation, "activity-link"),
            ] {
                for operation in [
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchOperation::Update,
                ] {
                    for borrowed in [false, true] {
                        for count in 0..=2 {
                            if count == 2 && operation == ProjectedBatchOperation::Update {
                                continue;
                            }
                            let projection = runtime_for_schema(&schema, "node-single-target.yaml");
                            let documents = (0..count)
                                .map(|index| {
                                    let mut document = serde_json::json!({
                                        "_iid": format!("0xa{}", index + 1),
                                        "_type": label,
                                        "attributes": {"tag": []},
                                    });
                                    if kind == TypeKind::Relation {
                                        document["attributes"] = serde_json::json!({});
                                        document["_role_0_iid"] = serde_json::json!("0xd1");
                                        document["_role_0_type"] =
                                            serde_json::json!("plain-activity");
                                        document["_role_0_attributes"] = serde_json::json!({});
                                    }
                                    document
                                })
                                .collect();
                            let (database, state) = projection_recording_database_for_test(vec![
                                ProjectionRecordingResponse::Query(QueryResult::Documents(
                                    documents,
                                )),
                            ]);
                            let runtime = Arc::new(ProviderRuntimeOwner::new().unwrap());
                            let transaction = borrowed.then(|| {
                                runtime
                                    .block_on(database.transaction_context(TxType::Write))
                                    .unwrap()
                            });
                            let manager = NodeProjectedModelManager {
                                package: Arc::clone(&projection.package),
                                type_id: TypeId::new(kind, label).unwrap(),
                                target: Some(match &transaction {
                                    Some(transaction) => {
                                        OwnedQueryTarget::Transaction(transaction.clone())
                                    }
                                    None => OwnedQueryTarget::Database(Arc::clone(&database)),
                                }),
                                successor_batch_marker: borrowed
                                    .then(|| Arc::new(AtomicBool::new(false))),
                                runtime: Arc::clone(&runtime),
                                filters: Vec::new(),
                                closed_filter: None,
                                has_empty_iids: false,
                            };
                            let roles = if kind == TypeKind::Relation {
                                vec![(
                                    RoleId::new(label, "subject").unwrap(),
                                    vec![
                                        ProjectedReference::try_new(
                                            &projection.package.projection,
                                            TypeId::new(TypeKind::Relation, "plain-activity")
                                                .unwrap(),
                                            Some("0xd1".into()),
                                            Vec::new(),
                                        )
                                        .unwrap(),
                                    ],
                                )]
                            } else {
                                Vec::new()
                            };
                            let instance = ProjectedCreate::try_new(
                                &projection.package.projection,
                                manager.type_id.clone(),
                                Vec::new(),
                                roles,
                            )
                            .unwrap();
                            let values = manager
                                .write_single_projected(
                                    instance,
                                    operation,
                                    (operation == ProjectedBatchOperation::Update)
                                        .then(|| "0xa1".into()),
                                )
                                .unwrap_or_else(|error| {
                                    panic!("{kind:?}/{operation:?}/{borrowed}/{count}: {error:?}")
                                });
                            assert_eq!(values.len(), count);
                            for (index, value) in values.iter().enumerate() {
                                assert_eq!(value.thing.iid(), format!("0xa{}", index + 1));
                                assert!(value.thing.origin_carrier().is_some());
                                let wire: ProjectedWire =
                                    serde_json::from_str(&value.json).unwrap();
                                assert_eq!(wire.iid.as_deref(), Some(value.thing.iid()));
                            }
                            {
                                let state = state.lock().unwrap();
                                assert_eq!(state.queries.len(), 1);
                                assert_eq!(state.commits, usize::from(!borrowed));
                                assert_eq!(state.rollbacks, 0);
                            }
                            if let Some(transaction) = transaction {
                                runtime.block_on(transaction.commit_sdk()).unwrap();
                                assert_eq!(state.lock().unwrap().commits, 1);
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn node_single_delete_uses_one_native_plan_for_absent_and_present_roots() {
        use type_bridge_orm::TxType;
        use type_bridge_orm::session::backend::QueryResult;

        for schema in [
            ORDERED_SCHEMA.to_owned(),
            ORDERED_SCHEMA
                .replace("        ordered: true\n", "")
                .replace("        distinct: true\n", ""),
        ] {
            for kind in [TypeKind::Entity, TypeKind::Relation] {
                for borrowed in [false, true] {
                    for present in [false, true] {
                        let projection = runtime_for_schema(&schema, "node-single-delete.yaml");
                        let rows = if present {
                            vec![
                                serde_json::json!({"ordinal0": {"kind": "value", "value": type_bridge_contract::value::CanonicalValue::Long(0)}}),
                            ]
                        } else {
                            vec![]
                        };
                        let (database, state) = projection_recording_database_for_test(vec![
                            ProjectionRecordingResponse::Query(QueryResult::Rows(rows)),
                        ]);
                        let runtime = Arc::new(ProviderRuntimeOwner::new().unwrap());
                        let transaction = borrowed.then(|| {
                            runtime
                                .block_on(database.transaction_context(TxType::Write))
                                .unwrap()
                        });
                        let marker = borrowed.then(|| Arc::new(AtomicBool::new(false)));
                        let manager = NodeProjectedModelManager {
                            package: Arc::clone(&projection.package),
                            type_id: TypeId::new(
                                kind,
                                if kind == TypeKind::Entity {
                                    "group"
                                } else {
                                    "activity-link"
                                },
                            )
                            .unwrap(),
                            target: Some(match &transaction {
                                Some(transaction) => {
                                    OwnedQueryTarget::Transaction(transaction.clone())
                                }
                                None => OwnedQueryTarget::Database(database),
                            }),
                            successor_batch_marker: marker.clone(),
                            runtime: Arc::clone(&runtime),
                            filters: Vec::new(),
                            closed_filter: None,
                            has_empty_iids: false,
                        };
                        manager
                            .delete_by_iid("0xa1".into())
                            .unwrap_or_else(|error| {
                                panic!(
                                    "{kind:?}/{borrowed}/{present}: {error:?}; {:?}",
                                    state.lock().unwrap().queries
                                )
                            });
                        {
                            let state = state.lock().unwrap();
                            assert_eq!(state.queries.len(), 1);
                            assert_eq!(state.commits, usize::from(!borrowed));
                            assert_eq!(state.rollbacks, 0);
                        }
                        if let Some(transaction) = transaction {
                            assert!(marker.unwrap().load(Ordering::Acquire));
                            runtime.block_on(transaction.commit_sdk()).unwrap();
                            assert_eq!(state.lock().unwrap().commits, 1);
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn node_returning_delete_retains_snapshot_origin_on_owned_and_borrowed_routes() {
        use type_bridge_orm::TxType;
        use type_bridge_orm::session::backend::QueryResult;

        for schema in [
            ORDERED_SCHEMA.to_owned(),
            ORDERED_SCHEMA
                .replace("        ordered: true\n", "")
                .replace("        distinct: true\n", ""),
        ] {
            for borrowed in [false, true] {
                let projection = runtime_for_schema(&schema, "node-native-returning-delete.yaml");
                let (database, state) = projection_recording_database_for_test(vec![
                    ProjectionRecordingResponse::Query(QueryResult::Documents(vec![
                        serde_json::json!({
                            "_iid": "0xa1", "_type": "group", "attributes": {}
                        }),
                    ])),
                    ProjectionRecordingResponse::Query(QueryResult::Rows(vec![serde_json::json!(
                        {}
                    )])),
                ]);
                let runtime = Arc::new(ProviderRuntimeOwner::new().unwrap());
                let transaction = borrowed.then(|| {
                    runtime
                        .block_on(database.transaction_context(TxType::Write))
                        .unwrap()
                });
                let marker = borrowed.then(|| Arc::new(AtomicBool::new(false)));
                let manager = NodeProjectedModelManager {
                    package: Arc::clone(&projection.package),
                    type_id: TypeId::new(TypeKind::Entity, "group").unwrap(),
                    target: Some(match &transaction {
                        Some(transaction) => OwnedQueryTarget::Transaction(transaction.clone()),
                        None => OwnedQueryTarget::Database(Arc::clone(&database)),
                    }),
                    successor_batch_marker: marker.clone(),
                    runtime: Arc::clone(&runtime),
                    filters: Vec::new(),
                    closed_filter: None,
                    has_empty_iids: false,
                };
                let mut snapshots = manager
                    .delete_returning_envelopes(vec!["0xa1".into()])
                    .unwrap_or_else(|error| {
                        panic!("{error:?}; queries={:?}", state.lock().unwrap().queries)
                    });
                assert_eq!(snapshots.len(), 1);
                let snapshot = snapshots.remove(0);
                assert!(snapshot.deleted());
                assert_eq!(snapshot.thing.iid(), "0xa1");
                assert_eq!(
                    snapshot
                        .thing
                        .try_to_reference(projection.package.projection.as_ref())
                        .unwrap_err()
                        .code()
                        .as_str(),
                    "thing_deleted"
                );
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.queries.len(), 2);
                    assert_eq!(state.commits, usize::from(!borrowed));
                    assert_eq!(state.rollbacks, 0);
                }
                if let Some(transaction) = transaction {
                    assert!(marker.unwrap().load(Ordering::Acquire));
                    runtime.block_on(transaction.commit_sdk()).unwrap();
                    assert_eq!(state.lock().unwrap().commits, 1);
                }
            }
        }
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn node_batch_routes_preserve_provider_and_hydration_atomicity() {
        use type_bridge_orm::TxType;
        use type_bridge_orm::session::backend::QueryResult;

        let schema = ORDERED_SCHEMA.replace(
            "      subject:\n        card: { min: 0, max: 4 }\n        ordered: true\n        distinct: true",
            "      subject:\n        card: { min: 0, max: 4 }",
        );
        for schema in [
            schema.clone(),
            schema
                .replace("        ordered: true\n", "")
                .replace("        distinct: true\n", ""),
        ] {
            let runtime_projection =
                runtime_for_schema(&schema, "node-unordered-batch-atomicity.yaml");
            let cases = [
                (
                    TypeId::new(TypeKind::Entity, "group").unwrap(),
                    ProjectedWire {
                        type_key: type_key(TypeKind::Entity, "group"),
                        form: WireForm::Complete,
                        iid: None,
                        value: None,
                        values: BTreeMap::from([("tag".into(), Value::Array(Vec::new()))]),
                    },
                    "group",
                    serde_json::json!({
                        "ordinal": 0,
                        "_iid": "0xa1",
                        "_type": "person",
                        "attributes": {"tag": []},
                    }),
                ),
                (
                    TypeId::new(TypeKind::Relation, "activity-link").unwrap(),
                    ProjectedWire {
                        type_key: type_key(TypeKind::Relation, "activity-link"),
                        form: WireForm::Complete,
                        iid: None,
                        value: None,
                        values: BTreeMap::from([(
                            "subject".into(),
                            Value::Array(vec![
                                serde_json::to_value(ProjectedWire {
                                    type_key: type_key(TypeKind::Relation, "plain-activity"),
                                    form: WireForm::Reference,
                                    iid: Some("0xd1".into()),
                                    value: None,
                                    values: BTreeMap::new(),
                                })
                                .unwrap(),
                            ]),
                        )]),
                    },
                    "activity-link",
                    serde_json::json!({
                        "ordinal": 0,
                        "_iid": "0xe1",
                        "_type": "plain-activity",
                        "attributes": {},
                        "_role_0_players": [{"_iid":"0xd1", "_type":"plain-activity", "attributes":{}}],
                    }),
                ),
            ];

            for (type_id, wire, provider_label, malformed_hydration) in cases {
                for (borrowed, rollback_first) in [(false, false), (true, false), (true, true)] {
                    for hydration_failure in [false, true] {
                        let relation = type_id.kind() == TypeKind::Relation;
                        let responses = if hydration_failure {
                            vec![ProjectionRecordingResponse::Query(QueryResult::Documents(
                                vec![malformed_hydration.clone()],
                            ))]
                        } else {
                            vec![ProjectionRecordingResponse::Failure {
                                provider_failure: true,
                            }]
                        };
                        let (database, state) = projection_recording_database_for_test(responses);
                        let provider_runtime = Arc::new(
                            ProviderRuntimeOwner::new()
                                .expect("provider runtime should start for Node route test"),
                        );
                        let transaction = borrowed.then(|| {
                            provider_runtime
                                .block_on(database.transaction_context(TxType::Write))
                                .unwrap()
                        });
                        if rollback_first {
                            provider_runtime
                                .block_on(transaction.as_ref().unwrap().rollback())
                                .unwrap();
                        }
                        let marker = borrowed.then(|| Arc::new(AtomicBool::new(false)));
                        let manager = NodeProjectedModelManager {
                            package: Arc::clone(&runtime_projection.package),
                            type_id: type_id.clone(),
                            target: Some(match &transaction {
                                Some(transaction) => {
                                    OwnedQueryTarget::Transaction(transaction.clone())
                                }
                                None => OwnedQueryTarget::Database(Arc::clone(&database)),
                            }),
                            successor_batch_marker: marker.clone(),
                            runtime: Arc::clone(&provider_runtime),
                            filters: Vec::new(),
                            closed_filter: None,
                            has_empty_iids: false,
                        };
                        let (batch, control) = one_row_insert_batch(
                            &runtime_projection,
                            type_id.clone(),
                            wire.clone(),
                        );
                        let diagnostic = manager
                            .execute_projected_batch(
                                &batch,
                                control,
                                false,
                                (),
                                |_| -> Result<_, _> {
                                    panic!(
                                        "provider/hydration failure unexpectedly reached the mapper"
                                    )
                                },
                            )
                            .unwrap_err();
                        if hydration_failure {
                            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
                            assert_eq!(
                                diagnostic.code().as_str(),
                                if relation {
                                    "query_remote_v3_document_type"
                                } else {
                                    "query_remote_v3_answer_kind"
                                },
                                "relation={relation}; queries={:?}",
                                state.lock().unwrap().queries
                            );
                        } else {
                            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Provider);
                        }

                        let state = state.lock().unwrap();
                        assert_eq!(state.opens, ["write"]);
                        assert_eq!(state.queries.len(), 1);
                        assert!(
                            state
                                .queries
                                .iter()
                                .any(|query| query.contains(provider_label))
                        );
                        assert_eq!(state.given_rows.len(), state.queries.len());
                        assert_eq!(state.commits, 0);
                        if borrowed {
                            assert_eq!(state.rollbacks, usize::from(rollback_first));
                            assert_eq!(state.closes, 0);
                        } else {
                            assert_eq!(state.rollbacks, 1);
                            assert_eq!(state.closes, 0);
                        }
                        drop(state);

                        if let (Some(transaction), Some(marker)) = (transaction, marker) {
                            assert!(marker.load(Ordering::Acquire));
                            assert_eq!(
                                provider_runtime.block_on(transaction.lifecycle_state()),
                                TransactionContextState::RollbackOnly
                            );
                            let transaction = crate::NodeRustTransactionContext {
                                context: transaction,
                                runtime: Arc::clone(&provider_runtime),
                                successor_batch_invoked: marker,
                            };
                            let commit = transaction.commit().unwrap_err();
                            let diagnostic: Value = serde_json::from_str(&commit.reason).unwrap();
                            assert_eq!(diagnostic["code"], "transaction_rollback_only");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn projected_batch_mapper_and_writer_keep_the_lease_safe_source_fence() {
        let source = include_str!("runtime_projection.rs");
        let activation = source
            .split_once("fn finish(mut self) {")
            .unwrap()
            .1
            .split_once("impl Drop for PendingBatchAuthority")
            .unwrap()
            .0;
        assert!(activation.contains(".store(BATCH_AUTHORITY_ACTIVE, Ordering::Release)"));
        for forbidden in ["debug_assert", "compare_exchange", "Err(", "?"] {
            assert!(
                !activation.contains(forbidden),
                "post-common activation crossed the infallible source fence: {forbidden}"
            );
        }

        let exception_bridge = source
            .split_once("fn napi_status_preserving_exception(")
            .unwrap()
            .1
            .split_once("fn raw_typeof(")
            .unwrap()
            .0;
        assert!(exception_bridge.contains("Error::from_unknown_without_coercion(value)"));
        assert!(!exception_bridge.contains("Error::from(value)"));

        let proof_parser = source
            .split_once("fn raw_projected_input_proofs(")
            .unwrap()
            .1
            .split_once("const MAX_PROJECTED_BINDING_JSON_BYTES")
            .unwrap()
            .0;
        assert!(proof_parser.contains("let scope = RawHandleScope::open(env)?;"));
        assert!(proof_parser.contains("scope.close()?;\n        proofs.push(proof);"));
        assert!(proof_parser.contains(".checkpoint()"));

        let proof_descriptor = source
            .split_once("fn raw_projected_input_proof(")
            .unwrap()
            .1
            .split_once("fn reserved_projected_proof_slots(")
            .unwrap()
            .0;
        assert!(proof_descriptor.contains(".role_ordinal(&role_name)"));
        assert!(proof_descriptor.contains(".role_id(role_ordinal)"));
        assert!(!proof_descriptor.contains(".enumerate()"));
        assert!(!proof_descriptor.contains(".find("));

        let proof_restore = source
            .split_once("impl NodeProjectedInputProof {")
            .unwrap()
            .1
            .split_once("pub struct NodeProjectedValueEnvelope")
            .unwrap()
            .0;
        assert!(proof_restore.contains("cache.roles.role_id(*role_ordinal)"));
        assert!(!proof_restore.contains(".nth("));

        let mapper = source
            .split_once("struct PreparedProjectedBatchOutput {")
            .unwrap()
            .1
            .split_once("#[derive(Clone)]\nenum FacadeProjectionProof")
            .unwrap()
            .0;
        for forbidden in [
            "Function::call",
            "External::new",
            "ThreadsafeFunction",
            "serde_json::to_",
            "projected_thing_wire(",
            ".validate_for(",
        ] {
            assert!(
                !mapper.contains(forbidden),
                "batch mapper crossed the forbidden source fence: {forbidden}"
            );
        }
        assert!(mapper.contains("sys::napi_call_function"));
        assert!(mapper.contains("sys::napi_get_and_clear_last_exception"));

        let writer = source
            .split_once("fn write_json_string(")
            .unwrap()
            .1
            .split_once("fn projected_create_wire(")
            .unwrap()
            .0;
        for forbidden in [
            "serde_json::",
            ".validate_for(",
            "format!(",
            ".to_owned()",
            "collect::<Vec",
        ] {
            assert!(
                !writer.contains(forbidden),
                "batch writer crossed the forbidden source fence: {forbidden}"
            );
        }
    }

    #[test]
    fn projected_batch_row_reservation_failure_uses_the_common_diagnostic() {
        FAIL_PROJECTED_BATCH_ROW_RESERVATION.store(true, Ordering::Relaxed);
        let diagnostic = reserved_projected_batch_rows::<ProjectedBatchRow>(3).unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_batch_allocation_exhausted"
        );
        assert!(matches!(
            diagnostic.path(),
            [SdkDiagnosticPathSegment::Argument(name)] if name.as_str() == "rows"
        ));
    }

    #[test]
    fn fallible_projected_scalar_writer_covers_js_temporal_and_all_scalar_domains() {
        let datetime: CanonicalDateTime = "2026-10-25T01:30:00.123456789".parse().unwrap();
        let named =
            CanonicalDateTimeTz::new_named_resolved(datetime, "Europe/London", 3_600).unwrap();
        let second_offset =
            CanonicalDateTimeTz::new_fixed(datetime, TimeZoneDesignator::OffsetSeconds(3_661))
                .unwrap();
        let values = [
            (
                CanonicalValue::String(CanonicalString::new("line\nvalue").unwrap()),
                serde_json::json!({"valueType":"string","value":"line\nvalue"}),
            ),
            (
                CanonicalValue::Long(9_007_199_254_740_993),
                serde_json::json!({"valueType":"long","value":"9007199254740993"}),
            ),
            (
                CanonicalValue::Double(CanonicalDouble::new(1.5).unwrap()),
                serde_json::json!({"valueType":"double","value":1.5}),
            ),
            (
                CanonicalValue::Boolean(true),
                serde_json::json!({"valueType":"boolean","value":true}),
            ),
            (
                CanonicalValue::Date("+10000-01-02".parse().unwrap()),
                serde_json::json!({"valueType":"date","value":"+010000-01-02"}),
            ),
            (
                CanonicalValue::DateTime("-9999-01-02T03:04:05.12".parse().unwrap()),
                serde_json::json!({"valueType":"datetime","value":"-009999-01-02T03:04:05.12"}),
            ),
            (
                CanonicalValue::DateTimeTz(named),
                serde_json::json!({"valueType":"datetime_tz","value":"2026-10-25T00:30:00.123456789Z"}),
            ),
            (
                CanonicalValue::DateTimeTz(second_offset),
                serde_json::json!({"valueType":"datetime_tz","value":"2026-10-25T00:28:59.123456789Z"}),
            ),
            (
                CanonicalValue::Decimal(DecimalValue::new("123.450").unwrap()),
                serde_json::json!({"valueType":"decimal","value":"123.45"}),
            ),
            (
                CanonicalValue::Duration("-P2M3DT4.12S".parse().unwrap()),
                serde_json::json!({"valueType":"duration","value":"-P2M3DT4.12S"}),
            ),
        ];
        for (value, expected) in values {
            let mut output = FallibleJsonWriter::default();
            write_projected_scalar(&mut output, &value).unwrap();
            let actual: Value = serde_json::from_slice(output.as_bytes()).unwrap();
            assert_eq!(actual, expected, "failed scalar domain {value:?}");
        }
    }

    #[test]
    fn projected_batch_writer_matches_existing_entity_and_nested_role_wires() {
        let runtime = ordered_runtime();
        let package = runtime.package.as_ref();
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let tag_a = attribute_wire("tag", ValueTypeTag::String, Value::String("alpha".into()));
        let tag_b = attribute_wire("tag", ValueTypeTag::String, Value::String("beta".into()));
        let person = project_thing_wire(
            package,
            &person_id,
            &ProjectedWire {
                type_key: canonical_type_key(&person_id).unwrap(),
                form: WireForm::Complete,
                iid: Some("0xa1".into()),
                value: None,
                values: BTreeMap::from([
                    (
                        "identifier".into(),
                        serde_json::to_value(identifier).unwrap(),
                    ),
                    ("score".into(), serde_json::to_value(score).unwrap()),
                    ("tag".into(), serde_json::to_value([tag_a, tag_b]).unwrap()),
                ]),
            },
        )
        .unwrap();

        let link_id = TypeId::new(TypeKind::Relation, "activity-link").unwrap();
        let player_id = TypeId::new(TypeKind::Relation, "plain-activity").unwrap();
        let link_model = package
            .projection
            .projection()
            .models()
            .get(&link_id)
            .unwrap();
        let (role_id, read_role) = link_model.complete_read().roles().iter().next().unwrap();
        assert!(read_role.players().iter().any(|player| {
            player.id() == &player_id && player.form() == ProjectedModelForm::Reference
        }));
        let reference = ProjectedReference::try_new_for_hydration(
            package.projection.as_ref(),
            player_id,
            Some("0xd1".into()),
            Vec::new(),
        )
        .unwrap();
        let player = ProjectedRolePlayer::try_new_reference_for_hydration(
            package.projection.as_ref(),
            read_role,
            reference,
        )
        .unwrap();
        let relation = ProjectedThing::try_new(
            package.projection.as_ref(),
            link_id,
            "0xe1".into(),
            Vec::new(),
            vec![(role_id.clone(), vec![player])],
        )
        .unwrap();

        let membership_id = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let membership_model = package
            .projection
            .projection()
            .models()
            .get(&membership_id)
            .unwrap();
        let (member_role_id, member_read_role) = membership_model
            .complete_read()
            .roles()
            .iter()
            .next()
            .unwrap();
        assert!(member_read_role.players().iter().any(|player| {
            player.id() == &person_id && player.form() == ProjectedModelForm::Complete
        }));
        let complete_player = ProjectedRolePlayer::try_new_complete_for_hydration(
            package.projection.as_ref(),
            member_read_role,
            person
                .try_to_reference(package.projection.as_ref())
                .unwrap(),
            person
                .fields()
                .iter()
                .map(|(field, values)| (field.clone(), values.clone()))
                .collect(),
        )
        .unwrap();
        let membership = ProjectedThing::try_new(
            package.projection.as_ref(),
            membership_id,
            "0xf1".into(),
            Vec::new(),
            vec![(member_role_id.clone(), vec![complete_player])],
        )
        .unwrap();

        for thing in [&person, &relation, &membership] {
            let existing =
                serde_json::to_value(projected_thing_wire(package, thing).unwrap()).unwrap();
            let mut output = FallibleJsonWriter::default();
            write_projected_thing_json(&mut output, package, thing).unwrap();
            let streamed: Value = serde_json::from_slice(output.as_bytes()).unwrap();
            assert_eq!(streamed, existing);
        }
    }

    #[test]
    fn ordered_scalar_admission_is_canonical_and_structured() {
        let runtime = ordered_runtime();
        let score = type_key(TypeKind::Attribute, "score");
        let overflow = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9223372036854775808".into()),
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(score, overflow)
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "invalid_input");
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");

        let double = type_key(TypeKind::Attribute, "val-double");
        let nonfinite = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Double,
            value: Value::Null,
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(double, nonfinite)
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");

        let datetime = type_key(TypeKind::Attribute, "val-datetime");
        runtime
            .validate_attribute_value_json(
                datetime,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTime,
                    value: Value::String("2026-07-29T01:02:03.120".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let datetime_tz = type_key(TypeKind::Attribute, "val-datetime-tz");
        runtime
            .validate_attribute_value_json(
                datetime_tz.clone(),
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTimeTz,
                    value: Value::String("2026-07-29T01:02:03.120Z".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let error = runtime
            .validate_attribute_value_json(
                datetime_tz,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTimeTz,
                    value: Value::String("2026-07-29T01:02:03.1200Z".into()),
                })
                .unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");
    }

    #[test]
    fn unordered_scalar_admission_retains_the_legacy_plain_error_contract() {
        let runtime = runtime_for_schema(UNORDERED_SCHEMA, "node-legacy-admission.yaml");
        let score = type_key(TypeKind::Attribute, "score");
        let overflow = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9223372036854775808".into()),
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(score, overflow)
            .unwrap_err();
        assert_eq!(error.reason, "long envelope is outside i64");
        assert!(serde_json::from_str::<Value>(&error.reason).is_err());
    }

    #[test]
    fn ordered_whole_create_rejects_duplicates_and_accepts_inherited_players() {
        let runtime = ordered_runtime();
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let person_key = type_key(TypeKind::Entity, "person");
        let duplicate_person = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                (
                    "tag".into(),
                    Value::Array(
                        [
                            serde_json::to_value(&tag).unwrap(),
                            serde_json::to_value(&tag).unwrap(),
                        ]
                        .into(),
                    ),
                ),
            ]),
        };
        let error = runtime
            .validate_create_json(
                person_key.clone(),
                serde_json::to_string(&duplicate_person).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
        assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");

        let out_of_range_person = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                (
                    "score".into(),
                    serde_json::to_value(attribute_wire(
                        "score",
                        ValueTypeTag::Long,
                        Value::String("6".into()),
                    ))
                    .unwrap(),
                ),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let error = runtime
            .validate_create_json(
                person_key.clone(),
                serde_json::to_string(&out_of_range_person).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "range_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");

        let identified_person = ProjectedWire {
            type_key: person_key,
            form: WireForm::Complete,
            iid: Some("0xa".into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let plain_activity_key = type_key(TypeKind::Relation, "plain-activity");
        let plain_activity = ProjectedWire {
            type_key: plain_activity_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "participant".into(),
                serde_json::to_value(&identified_person).unwrap(),
            )]),
        };
        runtime
            .validate_create_json(
                plain_activity_key,
                serde_json::to_string(&plain_activity).unwrap(),
            )
            .unwrap();

        let membership_key = type_key(TypeKind::Relation, "membership");
        let membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                Value::Array(
                    [
                        serde_json::to_value(&identified_person).unwrap(),
                        serde_json::to_value(&identified_person).unwrap(),
                    ]
                    .into(),
                ),
            )]),
        };
        let error = runtime
            .validate_create_json(
                membership_key.clone(),
                serde_json::to_string(&membership).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");

        let unidentified_person = ProjectedWire {
            type_key: type_key(TypeKind::Entity, "person"),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([("tag".into(), Value::Array(Vec::new()))]),
        };
        let unidentified_membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                Value::Array(vec![serde_json::to_value(unidentified_person).unwrap()]),
            )]),
        };
        let error = runtime
            .validate_create_json(
                membership_key,
                serde_json::to_string(&unidentified_membership).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "missing_reference_identity");
        assert_eq!(
            diagnostic["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|segment| segment["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["type", "role", "index", "type"]
        );
    }

    #[test]
    fn canonical_model_codecs_round_trip_exact_node_wires_and_archive() {
        let runtime = ordered_runtime();
        let tag_key = type_key(TypeKind::Attribute, "tag");
        let tag = attribute_wire(
            "tag",
            ValueTypeTag::String,
            Value::String("canonical".into()),
        );
        let tag_bytes = runtime
            .encode_attribute_json(tag_key.clone(), serde_json::to_string(&tag).unwrap())
            .unwrap();
        let controlled_tag_bytes = runtime
            .encode_record_json_controlled(
                "attribute".into(),
                tag_key.clone(),
                serde_json::to_string(&tag).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(controlled_tag_bytes.to_vec(), tag_bytes.to_vec());
        let controlled_tag_json = runtime
            .decode_record_json_controlled(
                "attribute".into(),
                tag_key.clone(),
                Buffer::from(tag_bytes.to_vec()),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&controlled_tag_json).unwrap(),
            serde_json::to_value(&tag).unwrap(),
        );
        let cancelled = NodeQueryCancellation::new();
        cancelled.cancel();
        let error = runtime
            .encode_record_json_controlled(
                "attribute".into(),
                tag_key.clone(),
                serde_json::to_string(&tag).unwrap(),
                Some(&cancelled),
                None,
                None,
                None,
                None,
                None,
            )
            .err()
            .expect("cancelled nominal encode is rejected");
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "projected_codec_cancelled");
        let decoded: ProjectedWire = serde_json::from_str(
            &runtime
                .decode_attribute_json(tag_key.clone(), Buffer::from(tag_bytes.to_vec()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.type_key, tag.type_key);
        assert_eq!(decoded.form, WireForm::Complete);
        assert_eq!(decoded.iid, None);
        assert_eq!(
            serde_json::to_value(decoded.value).unwrap(),
            serde_json::to_value(tag.value).unwrap()
        );
        assert!(decoded.values.is_empty());
        assert!(
            runtime
                .decode_attribute_json(
                    type_key(TypeKind::Attribute, "identifier"),
                    Buffer::from(tag_bytes.to_vec()),
                )
                .is_err()
        );
        let person_key = type_key(TypeKind::Entity, "person");
        let wire = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(attribute_wire(
                        "identifier",
                        ValueTypeTag::String,
                        Value::String("person-1".into()),
                    ))
                    .unwrap(),
                ),
                (
                    "score".into(),
                    serde_json::to_value(attribute_wire(
                        "score",
                        ValueTypeTag::Long,
                        Value::String("3".into()),
                    ))
                    .unwrap(),
                ),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let input = serde_json::to_string(&wire).unwrap();
        let create_bytes = runtime
            .encode_create_json(person_key.clone(), input)
            .unwrap();
        let decoded = runtime
            .decode_create_json(person_key.clone(), Buffer::from(create_bytes.to_vec()))
            .unwrap();
        let decoded: ProjectedWire = serde_json::from_str(&decoded).unwrap();
        assert_eq!(decoded.type_key, wire.type_key);
        assert_eq!(decoded.form, WireForm::Complete);
        assert_eq!(decoded.iid, None);
        assert_eq!(decoded.values, wire.values);

        let reference = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Reference,
            iid: None,
            value: None,
            values: BTreeMap::from([("identifier".into(), wire.values["identifier"].clone())]),
        };
        let reference_bytes = runtime
            .encode_reference_json(
                person_key.clone(),
                serde_json::to_string(&reference).unwrap(),
            )
            .unwrap();
        let decoded: ProjectedWire = serde_json::from_str(
            &runtime
                .decode_reference_json(person_key.clone(), Buffer::from(reference_bytes.to_vec()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.type_key, reference.type_key);
        assert_eq!(decoded.form, reference.form);
        assert_eq!(decoded.iid, reference.iid);
        assert_eq!(decoded.values, reference.values);

        let snapshot = ProjectedWire {
            iid: Some("0x1".into()),
            ..wire.clone()
        };
        let snapshot_bytes = runtime
            .encode_snapshot_json(
                person_key.clone(),
                serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        let decoded: ProjectedWire = serde_json::from_str(
            &runtime
                .decode_snapshot_json(person_key, Buffer::from(snapshot_bytes.to_vec()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.type_key, snapshot.type_key);
        assert_eq!(decoded.form, snapshot.form);
        assert_eq!(decoded.iid, snapshot.iid);
        assert_eq!(decoded.values, snapshot.values);

        let expected_records = [
            tag_bytes.to_vec(),
            create_bytes.to_vec(),
            reference_bytes.to_vec(),
            snapshot_bytes.to_vec(),
        ];
        let archive = runtime
            .encode_archive(expected_records.iter().cloned().map(Buffer::from).collect())
            .unwrap();
        let decoded = runtime.decode_archive(archive).unwrap();
        assert_eq!(
            decoded
                .iter()
                .map(|bytes| bytes.to_vec())
                .collect::<Vec<_>>(),
            expected_records
        );

        let diagnostic_code = |error: napi::Error| {
            serde_json::from_str::<Value>(&error.reason).unwrap()["code"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let records = || expected_records.iter().cloned().map(Buffer::from).collect();
        let archive = runtime.encode_archive(records()).unwrap();

        let cancelled = NodeQueryCancellation::new();
        cancelled.cancel();
        let error = runtime
            .decode_archive_controlled(
                Buffer::from(archive.to_vec()),
                Some(&cancelled),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .err()
            .expect("cancelled decode is rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_cancelled");

        let error = runtime
            .decode_archive_controlled(
                Buffer::from(archive.to_vec()),
                None,
                Some(0),
                None,
                None,
                None,
                None,
                None,
            )
            .err()
            .expect("expired decode is rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_deadline_exceeded");

        let error = runtime
            .decode_archive_controlled(
                Buffer::from(archive.to_vec()),
                None,
                None,
                Some(1),
                None,
                None,
                None,
                None,
            )
            .err()
            .expect("oversized input is rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_input_limit");

        let error = runtime
            .encode_archive_controlled(records(), None, None, None, Some(1), None, None, None)
            .err()
            .expect("oversized output is rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_output_limit");

        let error = runtime
            .encode_archive_controlled(records(), None, None, None, None, None, Some(1), None)
            .err()
            .expect("excess records are rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_member_limit");

        let error = runtime
            .decode_archive_controlled(
                Buffer::from(archive.to_vec()),
                None,
                None,
                None,
                None,
                Some(1),
                None,
                None,
            )
            .err()
            .expect("excess depth is rejected");
        assert_eq!(diagnostic_code(error), "projected_codec_depth_limit");
    }

    #[test]
    fn ordered_facade_proof_restoration_requires_exact_visible_iid_and_keys() {
        let runtime = ordered_runtime();
        let package = runtime.package.as_ref();
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let membership_id = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let membership = package
            .projection
            .projection()
            .models()
            .get(&membership_id)
            .unwrap();
        let (role_id, role) = membership.create().roles().iter().next().unwrap();
        let operation_path = [
            SdkDiagnosticPathSegment::Type(membership_id),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
            SdkDiagnosticPathSegment::Index(0),
        ];
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let person = ProjectedWire {
            type_key: canonical_type_key(&person_id).unwrap(),
            form: WireForm::Complete,
            iid: Some("0xa1".into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let visible =
            project_reference_wire(package, role.players(), &person, &operation_path).unwrap();
        assert!(visible.origin_carrier().is_none());

        let reference_proof = NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Reference(Arc::new(visible.clone())),
        };
        let restored = project_reference_wire_with_proof(
            package,
            role.players(),
            &person,
            &operation_path,
            Some(&reference_proof),
        )
        .unwrap();
        assert_eq!(restored, visible);

        let complete = project_thing_wire(package, &person_id, &person).unwrap();
        let complete_proof = NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Thing(Arc::new(complete)),
        };
        let restored = project_reference_wire_with_proof(
            package,
            role.players(),
            &person,
            &operation_path,
            Some(&complete_proof),
        )
        .unwrap();
        assert_eq!(restored, visible);

        let detached_proof = NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::DetachedSnapshot,
        };
        let error = project_reference_wire_with_proof(
            package,
            role.players(),
            &person,
            &operation_path,
            Some(&detached_proof),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "projected_snapshot_detached");

        let mut changed_iid = person.clone();
        changed_iid.iid = Some("0xa2".into());
        let error = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_iid,
            &operation_path,
            Some(&complete_proof),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "malformed_projected_create");

        let mut changed_key = person;
        changed_key.values.insert(
            "identifier".into(),
            serde_json::to_value(attribute_wire(
                "identifier",
                ValueTypeTag::String,
                Value::String("person-2".into()),
            ))
            .unwrap(),
        );
        let error = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_key,
            &operation_path,
            Some(&reference_proof),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "malformed_projected_create");

        let lookalike = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_key,
            &operation_path,
            None,
        )
        .unwrap();
        assert_ne!(lookalike, visible);
        assert!(lookalike.origin_carrier().is_none());
    }

    #[test]
    fn ordered_single_writes_validate_before_iid_and_execution_target_resolution() {
        let runtime = ordered_runtime();
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let person_key = canonical_type_key(&person_id).unwrap();
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let duplicate = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                (
                    "tag".into(),
                    Value::Array(vec![
                        serde_json::to_value(&tag).unwrap(),
                        serde_json::to_value(&tag).unwrap(),
                    ]),
                ),
            ]),
        };
        let duplicate_json = serde_json::to_string(&duplicate).unwrap();
        let manager = manager_without_execution_target(&runtime, person_id);
        let write = |operation, iid: Option<String>, wire: String| {
            let input = manager.projected_create_input(&wire, &[])?;
            manager
                .write_single_projected(input, operation, iid)
                .map(|_| ())
        };

        for error in [
            write(
                ProjectedBatchOperation::Insert,
                None,
                duplicate_json.clone(),
            )
            .unwrap_err(),
            write(ProjectedBatchOperation::Put, None, duplicate_json.clone()).unwrap_err(),
            write(
                ProjectedBatchOperation::Update,
                Some(String::new()),
                duplicate_json,
            )
            .unwrap_err(),
        ] {
            let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
            assert_eq!(diagnostic["sdk_category"], "invalid_input");
            assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
            assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
            assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");
        }

        let mut nonempty = duplicate.clone();
        nonempty.values.insert(
            "tag".into(),
            Value::Array(vec![serde_json::to_value(&tag).unwrap()]),
        );
        for error in [
            write(
                ProjectedBatchOperation::Insert,
                None,
                serde_json::to_string(&nonempty).unwrap(),
            )
            .unwrap_err(),
            write(
                ProjectedBatchOperation::Put,
                None,
                serde_json::to_string(&nonempty).unwrap(),
            )
            .unwrap_err(),
            write(
                ProjectedBatchOperation::Update,
                Some(String::new()),
                serde_json::to_string(&nonempty).unwrap(),
            )
            .unwrap_err(),
        ] {
            let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
            assert_eq!(diagnostic["sdk_category"], "unsupported_capability");
            assert_eq!(
                diagnostic["code"],
                "query_plan_ordered_collection_unsupported"
            );
        }

        let valid = ProjectedWire {
            type_key: person_key,
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let error = write(
            ProjectedBatchOperation::Insert,
            None,
            serde_json::to_string(&valid).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.reason, "projected manager has no execution target");
    }

    /// Shared with the Python glue's lowering test so both hosts pin the same
    /// diagnostics for the same forged counts.
    const LOWERING_SCHEMA: &str = r#"format: typebridge.schema/v2
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

    /// The canonical host form of a lowering count refusal on the
    /// `LOWERING_SCHEMA` models; the Python glue's test pins the same values.
    fn lowering_count_refusal(
        code: &str,
        message: &str,
        [owner, member]: [Value; 2],
        [actual, minimum, maximum]: [&str; 3],
    ) -> Value {
        json!({
            "version": 1,
            "category": "invalid_input",
            "sdk_category": "invalid_input",
            "query_category": null,
            "code": code,
            "message": message,
            "path": [{"kind": "type", "value": owner}, member],
            "details": {
                "actual_count": {"kind": "count", "value": actual},
                "minimum_count": {"kind": "count", "value": minimum},
                "maximum_count": {"kind": "count", "value": maximum},
            },
        })
    }

    fn lowering_refusal(error: napi::Error) -> Value {
        assert_eq!(error.status, Status::InvalidArg);
        serde_json::from_str(&error.reason).unwrap()
    }

    #[test]
    fn unordered_write_lowering_refuses_forged_member_counts_with_core_diagnostics() {
        let runtime = runtime_for_schema(LOWERING_SCHEMA, "node-lowering-cardinality.yaml");
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let person_key = canonical_type_key(&person_id).unwrap();
        let identifier = serde_json::to_value(attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        ))
        .unwrap();
        let alias = serde_json::to_value(attribute_wire(
            "aliases",
            ValueTypeTag::String,
            Value::String("ada".into()),
        ))
        .unwrap();
        let person_wire = |values: Vec<(&str, Value)>| {
            serde_json::to_string(&ProjectedWire {
                type_key: person_key.clone(),
                form: WireForm::Complete,
                iid: None,
                value: None,
                values: values
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value))
                    .collect(),
            })
            .unwrap()
        };
        let person = json!({"kind": "entity", "label": "person"});
        let person_field = |attribute: &str| {
            [
                person.clone(),
                json!({"kind": "field", "value": {"attribute": attribute, "owner": person}}),
            ]
        };
        let manager = manager_without_execution_target(&runtime, person_id);

        for (wire, expected) in [
            (
                person_wire(vec![
                    ("identifier", identifier.clone()),
                    ("aliases", Value::Array(vec![alias.clone(); 3])),
                ]),
                lowering_count_refusal(
                    "field_cardinality_violation",
                    "The projected ownership violates its exact cardinality",
                    person_field("aliases"),
                    ["3", "0", "2"],
                ),
            ),
            (
                person_wire(vec![("aliases", Value::Array(vec![alias.clone()]))]),
                lowering_count_refusal(
                    "missing_required_field",
                    "A required projected ownership has no value",
                    person_field("identifier"),
                    ["0", "1", "1"],
                ),
            ),
        ] {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Put,
                ProjectedBatchOperation::Update,
            ] {
                let error = manager
                    .projected_create_input(&wire, &[])
                    .and_then(|input| {
                        manager.write_single_projected(input, operation, Some("0xa1".into()))
                    })
                    .map(|_| ())
                    .unwrap_err();
                assert_eq!(lowering_refusal(error), expected);
            }
            let error = project_batch_create_json(
                manager.package.as_ref(),
                &manager.type_id,
                &wire,
                &[],
                0,
            )
            .unwrap_err();
            let mut expected = expected.clone();
            let path = expected["path"].as_array_mut().unwrap();
            path.insert(0, json!({"kind": "argument", "value": "rows"}));
            path.insert(1, json!({"kind": "index", "value": 0}));
            assert_eq!(lowering_refusal(napi_sdk_diagnostic(error)), expected);
        }

        let valid = person_wire(vec![
            ("identifier", identifier),
            ("aliases", Value::Array(vec![alias; 2])),
        ]);
        assert_eq!(
            manager
                .projected_create_input(&valid, &[])
                .and_then(|input| manager.write_single_projected(
                    input,
                    ProjectedBatchOperation::Insert,
                    None
                ))
                .map(|_| ())
                .unwrap_err()
                .reason,
            "projected manager has no execution target"
        );

        let container_id = TypeId::new(TypeKind::Relation, "container").unwrap();
        let event = serde_json::to_value(ProjectedWire {
            type_key: type_key(TypeKind::Relation, "event"),
            form: WireForm::Complete,
            iid: Some("0xe1".into()),
            value: None,
            values: BTreeMap::new(),
        })
        .unwrap();
        let container = serde_json::to_string(&ProjectedWire {
            type_key: canonical_type_key(&container_id).unwrap(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([("item".into(), Value::Array(vec![event; 3]))]),
        })
        .unwrap();
        let manager = manager_without_execution_target(&runtime, container_id);
        assert_eq!(
            lowering_refusal(
                manager
                    .projected_create_input(&container, &[None, None, None])
                    .and_then(|input| manager.write_single_projected(
                        input,
                        ProjectedBatchOperation::Insert,
                        None
                    ))
                    .map(|_| ())
                    .unwrap_err()
            ),
            lowering_count_refusal(
                "role_cardinality_violation",
                "The projected role violates its exact cardinality",
                [
                    json!({"kind": "relation", "label": "container"}),
                    json!({"kind": "role", "value": {"declaring_relation": "container", "label": "item"}}),
                ],
                ["3", "0", "2"],
            )
        );

        let membership_id = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let membership = serde_json::to_string(&ProjectedWire {
            type_key: canonical_type_key(&membership_id).unwrap(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([("member".into(), Value::Null)]),
        })
        .unwrap();
        let manager = manager_without_execution_target(&runtime, membership_id);
        assert_eq!(
            lowering_refusal(
                manager
                    .projected_create_input(&membership, &[])
                    .and_then(|input| manager.write_single_projected(
                        input,
                        ProjectedBatchOperation::Insert,
                        None
                    ))
                    .map(|_| ())
                    .unwrap_err()
            ),
            lowering_count_refusal(
                "missing_required_role",
                "A required projected role has no player",
                [
                    json!({"kind": "relation", "label": "membership"}),
                    json!({"kind": "role", "value": {"declaring_relation": "membership", "label": "member"}}),
                ],
                ["0", "1", "1"],
            )
        );
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn iid_read_redirect_preserves_provider_query_bytes() {
        use type_bridge_orm::session::backend::QueryResult;
        let projection = ordered_runtime();
        let model = TypeId::new(TypeKind::Entity, "person").unwrap();
        let (database, state) = projection_recording_database_for_test(vec![
            ProjectionRecordingResponse::Query(QueryResult::Documents(vec![])),
            ProjectionRecordingResponse::Query(QueryResult::Documents(vec![])),
        ]);
        let mut manager = manager_without_execution_target(&projection, model.clone());
        manager.target = Some(OwnedQueryTarget::Database(Arc::clone(&database)));
        assert!(
            manager
                .get_by_iid_projected("0xa1".into())
                .unwrap()
                .is_none()
        );
        let executor = ProjectedCrudExecutor::new(&projection.package.projection);
        assert!(
            manager
                .runtime
                .block_on(executor.get_by_iid_on_target(
                    QueryTarget::Database(&database),
                    &model,
                    "0xa1",
                    None
                ))
                .unwrap()
                .is_none()
        );
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 2);
        assert_eq!(state.queries[0], state.queries[1]);
        assert_eq!(state.commits, 0);
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn iid_delete_redirect_preserves_provider_query_bytes() {
        use type_bridge_orm::session::backend::QueryResult;
        let projection = ordered_runtime();
        let model = TypeId::new(TypeKind::Entity, "group").unwrap();
        let (database, state) = projection_recording_database_for_test(vec![
            ProjectionRecordingResponse::Query(QueryResult::Rows(vec![])),
            ProjectionRecordingResponse::Query(QueryResult::Rows(vec![])),
        ]);
        let mut manager = manager_without_execution_target(&projection, model.clone());
        manager.target = Some(OwnedQueryTarget::Database(database));
        manager.delete_by_iid("0xa1".into()).unwrap();
        let control =
            ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
        let batch = ProjectedBatch::try_single_delete(
            &projection.package.projection,
            model,
            "0xa1".into(),
            &control,
        )
        .unwrap();
        manager
            .execute_projected_batch(&batch, control, false, (), |result| match result {
                ProjectedBatchResult::Deleted => Ok(()),
                _ => panic!("delete returned values"),
            })
            .unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 2);
        assert_eq!(state.queries[0], state.queries[1]);
        assert_eq!(state.commits, 2);
    }

    #[test]
    fn unordered_projected_read_reaches_native_target_resolution() {
        let runtime = runtime_for_schema(UNORDERED_SCHEMA, "node-legacy-read.yaml");
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let manager = manager_without_execution_target(&runtime, person_id);

        let error = match manager.get_by_iid_projected("0xa1".into()) {
            Err(error) => error,
            Ok(_) => panic!("unordered projected read must fail"),
        };

        assert_eq!(
            error.reason,
            "projected manager filter has no execution target"
        );
    }

    #[test]
    fn ordered_hydration_uses_common_integrity_validation_for_fields_and_roles() {
        let runtime = ordered_runtime();
        let tag_key = type_key(TypeKind::Attribute, "tag");
        runtime
            .validate_hydrated_attribute_value_json(
                tag_key.clone(),
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::String,
                    value: Value::String("same".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let error = runtime
            .validate_hydrated_attribute_value_json(
                tag_key,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::String,
                    value: Value::String("INVALID".into()),
                })
                .unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "regex_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");

        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let person_key = type_key(TypeKind::Entity, "person");
        let person = |iid: &str, score: ProjectedWire, tags: Vec<ProjectedWire>| ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: Some(iid.into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), serde_json::to_value(tags).unwrap()),
            ]),
        };

        let duplicate = person("0xa1", score.clone(), vec![tag.clone(), tag]);
        let error = runtime
            .validate_thing_json(
                person_key.clone(),
                serde_json::to_string(&duplicate).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");
        assert_eq!(diagnostic["path"][2]["kind"], "index");
        assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
        assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");

        let out_of_range = person(
            "0xa2",
            attribute_wire("score", ValueTypeTag::Long, Value::String("6".into())),
            Vec::new(),
        );
        let error = runtime
            .validate_thing_json(
                person_key.clone(),
                serde_json::to_string(&out_of_range).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "range_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");

        let reference = person("0xa1", score, Vec::new());
        let membership_key = type_key(TypeKind::Relation, "membership");
        let membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xb1".into()),
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                serde_json::to_value([reference.clone(), reference]).unwrap(),
            )]),
        };
        let error = runtime
            .validate_thing_json(membership_key, serde_json::to_string(&membership).unwrap())
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["path"][1]["kind"], "role");
        assert_eq!(diagnostic["path"][2]["kind"], "index");
    }

    #[test]
    fn ordered_hydration_accepts_effective_and_polymorphic_members_and_fences_iids() {
        let runtime = ordered_runtime();
        let group_id = TypeId::new(TypeKind::Entity, "group").unwrap();
        let group_key = canonical_type_key(&group_id).unwrap();
        let group = ProjectedWire {
            type_key: group_key,
            form: WireForm::Complete,
            iid: Some("0xc1".into()),
            value: None,
            values: BTreeMap::from([("tag".into(), Value::Array(Vec::new()))]),
        };
        let activity_key = type_key(TypeKind::Relation, "plain-activity");
        let activity = ProjectedWire {
            type_key: activity_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xd1".into()),
            value: None,
            values: BTreeMap::from([("participant".into(), serde_json::to_value(group).unwrap())]),
        };
        runtime
            .validate_thing_json(
                activity_key.clone(),
                serde_json::to_string(&activity).unwrap(),
            )
            .unwrap();

        let activity_reference = ProjectedWire {
            type_key: activity_key,
            form: WireForm::Reference,
            iid: Some("0xd1".into()),
            value: None,
            values: BTreeMap::new(),
        };
        let link_key = type_key(TypeKind::Relation, "activity-link");
        let link = ProjectedWire {
            type_key: link_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xe1".into()),
            value: None,
            values: BTreeMap::from([(
                "subject".into(),
                serde_json::to_value([activity_reference]).unwrap(),
            )]),
        };
        runtime
            .validate_thing_json(link_key, serde_json::to_string(&link).unwrap())
            .unwrap();

        let invalid = ProjectedWire {
            iid: Some("group-c1".into()),
            ..activity
        };
        let error = runtime
            .validate_thing_json(
                type_key(TypeKind::Relation, "plain-activity"),
                serde_json::to_string(&invalid).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "noncanonical_hydrated_iid");
        assert_eq!(diagnostic["path"][1]["kind"], "argument");
    }

    #[test]
    fn foreign_member_diagnostic_has_fixed_message_and_operation_path() {
        let runtime = ordered_runtime();
        let membership = type_key(TypeKind::Relation, "membership");
        let error = runtime
            .reject_generated_token_package_mismatch(
                serde_json::json!([
                    {"kind": "type", "typeKey": membership},
                    {"kind": "role", "name": "member"},
                    {"kind": "index", "value": 1},
                ])
                .to_string(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["category"], "integrity");
        assert_eq!(diagnostic["sdk_category"], "integrity");
        assert_eq!(diagnostic["code"], "generated_token_package_mismatch");
        assert_eq!(
            diagnostic["message"],
            "The generated token belongs to a different installed schema package"
        );
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "role");
        assert_eq!(
            diagnostic["path"][2],
            serde_json::json!({"kind": "index", "value": 1})
        );
    }

    #[cfg(feature = "contract-test-adapter")]
    #[test]
    fn schema_less_native_typed_queries_require_authority_before_provider_io() {
        let authority = authority(UNORDERED_SCHEMA, "node-schema-less-query.yaml");
        let projection = typescript_projection(&authority);
        let (json, semantic, fingerprint) = evidence(&projection);
        let installed = NodeRuntimeProjection::new(
            json,
            semantic,
            fingerprint,
            registrations(&projection),
            None,
            None,
        )
        .unwrap();
        let (database, state) = projection_recording_database_for_test(vec![]);
        let runtime = ProviderRuntimeOwner::new().unwrap();
        let registry = Arc::new(installed.package.projection.match_registry().unwrap());
        let session = type_bridge_orm::SessionHandle::new(Arc::clone(&registry));
        let root = session.exact("person").unwrap();
        let query = session
            .query(session.positional(vec![root.one()]).unwrap())
            .unwrap();
        let request = query.validate_count_by(&root).unwrap();
        let error = runtime
            .block_on(database.execute_match(&registry, &request))
            .unwrap_err();
        let type_bridge_orm::OrmError::Match(error) = error else {
            panic!("typed authority refusal")
        };
        assert_eq!(error.code().as_str(), "schema_authority_required");
        let state = state.lock().unwrap();
        assert!(state.queries.is_empty());
        assert!(state.opens.is_empty());
    }

    #[test]
    fn authorityless_install_retains_exact_unordered_legacy_admission_errors() {
        let legacy_authority = authority(UNORDERED_SCHEMA, "node-legacy-install.yaml");
        let legacy = typescript_projection(&legacy_authority);
        let (legacy_json, legacy_semantic, legacy_fingerprint) = evidence(&legacy);
        NodeRuntimeProjection::new(
            legacy_json.clone(),
            legacy_semantic.clone(),
            legacy_fingerprint.clone(),
            registrations(&legacy),
            None,
            None,
        )
        .expect("exact legacy unordered evidence must remain authorityless");

        let malformed = NodeRuntimeProjection::new(
            "{".into(),
            legacy_semantic,
            legacy_fingerprint,
            "[]".into(),
            None,
            None,
        )
        .err()
        .expect("malformed legacy projection evidence must fail");
        assert_eq!(
            malformed.reason,
            "invalid_contract [malformed_canonical_json]: input is not valid canonical JSON"
        );

        let emitter = TypeScriptEmitter::new();
        let mut forged_resources = emitter.code_resources().unwrap();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged legacy resource").unwrap();
        let forged = project(
            legacy_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers(),
            &forged_resources,
        )
        .unwrap();
        let (projection, semantic, fingerprint) = evidence(&forged);
        let forged =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None, None)
                .err()
                .expect("forged legacy resource evidence must fail");
        assert_eq!(
            forged.reason,
            "legacy TypeScript runtime projection does not match the exact shipped handler and resource evidence"
        );

        let ordered_authority = authority(ORDERED_SCHEMA, "node-legacy-ordered-install.yaml");
        let ordered = typescript_projection(&ordered_authority);
        let (projection, semantic, fingerprint) = evidence(&ordered);
        let missing_authority =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None, None)
                .err()
                .expect("authorityless ordered projection evidence must fail");
        assert_projection_evidence_mismatch(missing_authority);

        let authority = authority(ORDERED_SCHEMA, "node-foreign-target.yaml");
        let emitter = PythonEmitter::new();
        let projection = project(
            authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let (projection, semantic, fingerprint) = evidence(&projection);
        let error =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None, None)
                .err()
                .expect("a Python projection must not install as TypeScript");
        assert_eq!(
            error.reason,
            "runtime projection does not target TypeScript"
        );
    }

    #[test]
    fn public_successor_install_requires_one_materializer() {
        let authority = authority(ORDERED_SCHEMA, "node-successor-materializer.yaml");
        let authority_json = String::from_utf8(encode_schema_authority(&authority)).unwrap();
        let projection = typescript_projection(&authority);
        let registrations = registrations(&projection);
        let (projection, semantic, fingerprint) = evidence(&projection);

        let error = match NodeRuntimeProjection::new(
            projection,
            semantic,
            fingerprint,
            registrations,
            Some(authority_json),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("successor install without a materializer unexpectedly succeeded"),
        };
        assert_eq!(
            error.reason,
            "native projection requires exactly one generated batch materializer"
        );
    }

    #[test]
    fn successor_install_converges_all_hostile_evidence_on_the_integrity_diagnostic() {
        let ordered_authority = authority(ORDERED_SCHEMA, "node-ordered.yaml");
        let authority_json =
            String::from_utf8(encode_schema_authority(&ordered_authority)).unwrap();
        let exact = typescript_projection(&ordered_authority);
        let emitter = TypeScriptEmitter::new();
        assert_eq!(
            exact.generator_handlers(),
            [ProjectionHandler::typescript_v2()]
        );

        let (exact_json, exact_semantic, exact_fingerprint) = evidence(&exact);
        NodeRuntimeProjection::install(
            exact_json.clone(),
            exact_semantic.clone(),
            exact_fingerprint.clone(),
            registrations(&exact),
            Some(authority_json.clone()),
            None,
            false,
        )
        .expect("the exact ordered TypeScript package evidence must install");

        let rejected = |projection_json: String,
                        semantic: String,
                        fingerprint: String,
                        authority_json: Option<&str>| {
            NodeRuntimeProjection::new(
                projection_json,
                semantic,
                fingerprint,
                "[]".into(),
                authority_json.map(str::to_owned),
                None,
            )
            .err()
            .expect("hostile ordered projection evidence must fail")
        };

        let mut missing_resources = exact.code_resources().to_vec();
        missing_resources.pop();
        let missing = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &[ProjectionHandler::typescript_v2()],
            &missing_resources,
        )
        .unwrap();

        let mut raw: Value = serde_json::from_str(&exact_json).unwrap();
        let resources = raw["code_resources"].as_array().unwrap();
        let first_resource = resources[0].clone();

        let mut extra: Value = serde_json::from_str(&exact_json).unwrap();
        let mut extra_resource = first_resource.clone();
        extra_resource["id"] =
            Value::String("typebridge.generator.typescript.zzz-extra-resource".to_owned());
        extra["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(extra_resource);
        let extra = String::from_utf8(to_canonical_json(&extra).unwrap()).unwrap();

        let mut duplicate: Value = serde_json::from_str(&exact_json).unwrap();
        duplicate["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(first_resource);
        let duplicate = String::from_utf8(to_canonical_json(&duplicate).unwrap()).unwrap();

        let stale = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &[ProjectionHandler::typescript_v1()],
            &emitter.code_resources().unwrap(),
        )
        .unwrap();

        let mut forged_resources = exact.code_resources().to_vec();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged TypeScript resource").unwrap();
        let forged = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &[ProjectionHandler::typescript_v2()],
            &forged_resources,
        )
        .unwrap();

        raw["code_resources"].as_array_mut().unwrap().reverse();
        let reordered = String::from_utf8(to_canonical_json(&raw).unwrap()).unwrap();

        let foreign = authority(
            "format: typebridge.schema/v2\nentities:\n  foreign: {}\n",
            "node-foreign.yaml",
        );
        let foreign = String::from_utf8(encode_schema_authority(&foreign)).unwrap();

        let python = PythonEmitter::new();
        let foreign_target = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &python.generator_handlers_for(ordered_authority.resolved_schema()),
            &python
                .code_resources_for(ordered_authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let (foreign_target, foreign_target_semantic, foreign_target_fingerprint) =
            evidence(&foreign_target);

        let (missing, missing_semantic, missing_fingerprint) = evidence(&missing);
        let (stale, stale_semantic, stale_fingerprint) = evidence(&stale);
        let (forged, forged_semantic, forged_fingerprint) = evidence(&forged);

        // The string-based install boundary translates only empty bytes into
        // absence of the first detached evidence slot. The shared classifier
        // owns the resulting identity, index, counts, and provenance fact.
        assert_missing_semantic_fingerprint(rejected(
            exact_json.clone(),
            String::new(),
            exact_fingerprint.clone(),
            Some(&authority_json),
        ));

        for error in [
            rejected(
                "{".into(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                "{".into(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                "{".into(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some("{"),
            ),
            rejected(
                missing,
                missing_semantic,
                missing_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                extra,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                duplicate,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                reordered,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                stale,
                stale_semantic,
                stale_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                forged,
                forged_semantic,
                forged_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&foreign),
            ),
            rejected(
                foreign_target,
                foreign_target_semantic,
                foreign_target_fingerprint,
                Some(&authority_json),
            ),
        ] {
            assert_projection_evidence_mismatch(error);
        }
    }
}
