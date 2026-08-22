//! Candidate additive ABI 1.5 administration and migration boundary.
//!
//! This module is compiled only with the non-default `abi-1-5` feature until
//! its complete symbol, layout, compiler, and installed-consumer inventory is
//! frozen and activated atomically.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};

use crate::abi::{
    TypeBridgeByteView, TypeBridgeDiagnostics, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded,
};
use crate::diagnostic::diagnostics_handle;
use crate::generated_preflight::direct_output_preflight;
use crate::migration_runtime::{
    DatabaseAdministrationState, DatabaseDeletionPlanState, MigrationCatalogState,
    MigrationHistoryEntryState, MigrationIdentitySnapshot,
};
use crate::runtime::TypeBridgeDatabase;

/// Opaque bound-database administration owner.
pub struct TypeBridgeDatabaseAdministration {
    state: Arc<DatabaseAdministrationState>,
}

/// Opaque single-use pair-aware database deletion plan.
pub struct TypeBridgeDatabaseDeletionPlan {
    state: DatabaseDeletionPlanState,
}

/// Opaque immutable verified migration catalog.
pub struct TypeBridgeMigrationCatalog {
    state: Arc<MigrationCatalogState>,
}

/// Opaque independently owned migration-history entry snapshot.
pub struct TypeBridgeMigrationHistoryEntry {
    state: MigrationHistoryEntryState,
}

/// Opaque independently owned compound migration identity.
pub struct TypeBridgeMigrationIdentity {
    state: MigrationIdentitySnapshot,
}

const PAIR_ABSENT: u32 = 1;
const PAIR_STANDALONE_MANAGED: u32 = 2;
const PAIR_OWNED: u32 = 3;
const PAIR_OWNED_JOURNAL_ORPHAN: u32 = 4;
const CREATE_CREATED: u32 = 1;
const CREATE_ALREADY_EXISTS: u32 = 2;
const DELETE_ALREADY_ABSENT: u32 = 1;
const DELETE_STANDALONE_MANAGED: u32 = 2;
const DELETE_OWNED_PAIR: u32 = 3;
const DELETE_OWNED_JOURNAL_ORPHAN: u32 = 4;

fn failure_status(diagnostic: &Diagnostic) -> TypeBridgeStatus {
    match diagnostic.category() {
        DiagnosticCategory::InvalidContract => TypeBridgeStatus::InvalidArgument,
        DiagnosticCategory::UnsupportedCapability => TypeBridgeStatus::Unsupported,
        DiagnosticCategory::ResourceLimit => TypeBridgeStatus::ResourceLimit,
        DiagnosticCategory::Cancelled => TypeBridgeStatus::Cancelled,
        DiagnosticCategory::Integrity => TypeBridgeStatus::ExecutionFailed,
    }
}

unsafe fn initialize_diagnostics(
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: output preflight proved this complete caller slot writable.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    Ok(())
}

fn return_failure(
    diagnostic: Diagnostic,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let status = failure_status(&diagnostic);
    // SAFETY: every caller initializes and retains this writable output slot.
    unsafe {
        out_diagnostics.write_unaligned(Box::into_raw(Box::new(diagnostics_handle(vec![
            diagnostic,
        ]))))
    };
    status
}

unsafe fn preflight_one_output<T, Input>(
    input: *const Input,
    out: *mut T,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    let outputs = direct_output_preflight(&[
        (out.cast(), size_of::<T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeDiagnostics>(),
        ),
    ])?;
    outputs.check_bytes(input.cast(), size_of::<Input>())?;
    if out.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: output preflight proved the diagnostic slot writable and disjoint.
    unsafe { initialize_diagnostics(out_diagnostics) }
}

/// Open administration authority bound to the database handle's exact identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_open(
    database: *const TypeBridgeDatabase,
    out_administration: *mut *mut TypeBridgeDatabaseAdministration,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(database, out_administration, out_diagnostics) }
    {
        return status;
    }
    // SAFETY: the caller-provided handle slot was preflighted above.
    unsafe { out_administration.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(database) = (unsafe { database.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match DatabaseAdministrationState::open(database) {
            Ok(state) => {
                // SAFETY: the initialized output slot remains caller-owned.
                unsafe {
                    out_administration.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeDatabaseAdministration { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return whether the exact bound managed database currently exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_exists(
    administration: *const TypeBridgeDatabaseAdministration,
    out_exists: *mut u8,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(administration, out_exists, out_diagnostics) }
    {
        return status;
    }
    // SAFETY: output preflight proved this scalar writable.
    unsafe { out_exists.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.database_exists_blocking() {
            Ok(value) => {
                // SAFETY: initialized output slot remains caller-owned.
                unsafe { out_exists.write_unaligned(u8::from(value)) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Create the exact bound managed database and return a normalized outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_create(
    administration: *const TypeBridgeDatabaseAdministration,
    out_outcome: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(administration, out_outcome, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_outcome.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.create_database_outcome_blocking() {
            Ok(outcome) => {
                let value = match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created => CREATE_CREATED,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::AlreadyExists => CREATE_ALREADY_EXISTS,
                };
                unsafe { out_outcome.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Inspect the exact managed/journal pair state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_inspect(
    administration: *const TypeBridgeDatabaseAdministration,
    out_state: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(administration, out_state, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_state.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.inspect_blocking() {
            Ok(state) => {
                let value = pair_state(state);
                unsafe { out_state.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Build a single-use pair-aware destructive admission plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_plan_delete(
    administration: *const TypeBridgeDatabaseAdministration,
    out_plan: *mut *mut TypeBridgeDatabaseDeletionPlan,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(administration, out_plan, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_plan.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.plan_delete_blocking() {
            Ok(state) => {
                unsafe {
                    out_plan.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeDatabaseDeletionPlan { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return the pair state captured by a deletion plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_inspected_state(
    plan: *const TypeBridgeDatabaseDeletionPlan,
    out_state: *mut u32,
) -> TypeBridgeStatus {
    guarded(|| {
        if plan.is_null() || out_state.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(state) = (unsafe { &*plan }).state.inspected_state() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe { out_state.write_unaligned(pair_state(state)) };
        TypeBridgeStatus::Ok
    })
}

/// Consume a deletion plan and return the exact normalized terminal outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_execute(
    plan: *mut TypeBridgeDatabaseDeletionPlan,
    out_outcome: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(plan, out_outcome, out_diagnostics) } {
        return status;
    }
    unsafe { out_outcome.write_unaligned(0) };
    guarded(|| {
        let Some(plan) = (unsafe { plan.as_mut() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match plan.state.execute_blocking() {
            Ok(outcome) => {
                let value = match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::AlreadyAbsent => DELETE_ALREADY_ABSENT,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedStandaloneManaged => DELETE_STANDALONE_MANAGED,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedPair => DELETE_OWNED_PAIR,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedJournalOrphan => DELETE_OWNED_JOURNAL_ORPHAN,
                };
                unsafe { out_outcome.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Close a deletion plan idempotently and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_close(
    plan: *mut *mut TypeBridgeDatabaseDeletionPlan,
) -> TypeBridgeStatus {
    // SAFETY: this function forwards the caller-owned handle slot unchanged.
    unsafe { close_box(plan) }
}

/// Close administration unless a deletion plan still retains it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_close(
    administration: *mut *mut TypeBridgeDatabaseAdministration,
) -> TypeBridgeStatus {
    guarded(|| {
        if administration.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { administration.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        if Arc::strong_count(&unsafe { &*value }.state) != 1 {
            return TypeBridgeStatus::InUse;
        }
        unsafe {
            administration.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

fn pair_state(state: type_bridge_schema_migration_typedb::ManagedDatabasePairState) -> u32 {
    match state {
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::Absent => PAIR_ABSENT,
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::StandaloneManaged => {
            PAIR_STANDALONE_MANAGED
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedPair => PAIR_OWNED,
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedJournalOrphan => {
            PAIR_OWNED_JOURNAL_ORPHAN
        }
    }
}

unsafe fn close_box<T>(slot: *mut *mut T) -> TypeBridgeStatus {
    guarded(|| {
        if slot.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { slot.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        unsafe {
            slot.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

/// Open canonical bundle bytes under one verified generated package authority.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_open(
    package: *const TypeBridgeSchemaPackage,
    bundle: TypeBridgeByteView,
    out_catalog: *mut *mut TypeBridgeMigrationCatalog,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            out_catalog.cast(),
            size_of::<*mut TypeBridgeMigrationCatalog>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if outputs
        .check_bytes(package.cast(), size_of::<TypeBridgeSchemaPackage>())
        .and_then(|()| outputs.check_bytes(bundle.data.cast(), bundle.length))
        .is_err()
        || out_catalog.is_null()
    {
        return TypeBridgeStatus::InvalidArgument;
    }
    if let Err(status) = unsafe { initialize_diagnostics(out_diagnostics) } {
        return status;
    }
    unsafe { out_catalog.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(package) = (unsafe { package.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let bytes = match unsafe {
            bundle.snapshot(
                "migration_history_bundle",
                type_bridge_schema_migration::MAX_MIGRATION_HISTORY_BUNDLE_BYTES,
            )
        } {
            Ok(value) => value,
            Err((status, diagnostics)) => {
                unsafe {
                    out_diagnostics
                        .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
                };
                return status;
            }
        };
        match MigrationCatalogState::open(&package.state()._authority, &bytes) {
            Ok(state) => {
                unsafe {
                    out_catalog.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeMigrationCatalog { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Borrow canonical fingerprint JSON from a live catalog.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_fingerprint(
    catalog: *const TypeBridgeMigrationCatalog,
    out_fingerprint: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_fingerprint.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let bytes = unsafe { &*catalog }.state.fingerprint_json();
        unsafe {
            out_fingerprint.write_unaligned(TypeBridgeByteView {
                data: bytes.as_ptr(),
                length: bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded number of topologically ordered history entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_count(
    catalog: *const TypeBridgeMigrationCatalog,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(catalog, out_count, |value| value.state.len()) }
}

/// Return an independently owned history entry at one topological ordinal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_entry_at(
    catalog: *const TypeBridgeMigrationCatalog,
    index: usize,
    out_entry: *mut *mut TypeBridgeMigrationHistoryEntry,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_entry.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_entry.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*catalog }.state.entry(index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_entry.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationHistoryEntry {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded number of canonical graph heads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_head_count(
    catalog: *const TypeBridgeMigrationCatalog,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(catalog, out_count, |value| value.state.heads().len()) }
}

/// Return an independently owned canonical graph-head identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_head_at(
    catalog: *const TypeBridgeMigrationCatalog,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*catalog }.state.heads().get(index).cloned() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close a catalog only after every entry or plan child has closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_close(
    catalog: *mut *mut TypeBridgeMigrationCatalog,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { catalog.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        if Arc::strong_count(&unsafe { &*value }.state) != 1 {
            return TypeBridgeStatus::InUse;
        }
        unsafe {
            catalog.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

/// Return an independently owned identity for one history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_identity(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    unsafe { owned_identity(entry, out_identity, |entry| entry.state.identity()) }
}

/// Return the number of exact parent identities for one history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_parent_count(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.parents().len()) }
}

/// Return an independently owned exact parent identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_parent_at(
    entry: *const TypeBridgeMigrationHistoryEntry,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if entry.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*entry }.state.parents().get(index).cloned() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Copy the exact 32-byte canonical manifest digest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_manifest_digest(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_digest: *mut u8,
    digest_length: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        if entry.is_null() || out_digest.is_null() || digest_length != 32 {
            return TypeBridgeStatus::InvalidArgument;
        }
        let digest = unsafe { &*entry }.state.manifest_digest();
        unsafe { ptr::copy_nonoverlapping(digest.as_ptr(), out_digest, digest.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Return the canonical step count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_step_count(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.step_count()) }
}

/// Return the stable migration safety-class tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_safety(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_safety: *mut u32,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_safety, |value| safety(value.state.safety())) }
}

/// Return exact zero or one for verified reversibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_reversible(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_reversible: *mut u8,
) -> TypeBridgeStatus {
    unsafe {
        scalar(entry, out_reversible, |value| {
            u8::from(value.state.reversible())
        })
    }
}

/// Close a history-entry snapshot idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_close(
    entry: *mut *mut TypeBridgeMigrationHistoryEntry,
) -> TypeBridgeStatus {
    unsafe { close_box(entry) }
}

/// Borrow the application-label component of a live migration identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_app_label(
    identity: *const TypeBridgeMigrationIdentity,
    out_label: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    unsafe { identity_view(identity, out_label, |value| &value.state.app_label) }
}

/// Borrow the migration-name component of a live migration identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_name(
    identity: *const TypeBridgeMigrationIdentity,
    out_name: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    unsafe { identity_view(identity, out_name, |value| &value.state.name) }
}

/// Close a migration identity idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_close(
    identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    unsafe { close_box(identity) }
}

fn safety(value: type_bridge_schema::SafetyClass) -> u32 {
    match value {
        type_bridge_schema::SafetyClass::FormalOnly => 1,
        type_bridge_schema::SafetyClass::SchemaMetadata => 2,
        type_bridge_schema::SafetyClass::Additive => 3,
        type_bridge_schema::SafetyClass::Conditional => 4,
        type_bridge_schema::SafetyClass::BackfillRequired => 5,
        type_bridge_schema::SafetyClass::Destructive => 6,
        type_bridge_schema::SafetyClass::Opaque => 7,
        type_bridge_schema::SafetyClass::Unsupported => 8,
    }
}

unsafe fn scalar<Input, Output: Copy>(
    input: *const Input,
    out: *mut Output,
    read: impl FnOnce(&Input) -> Output,
) -> TypeBridgeStatus {
    guarded(|| {
        if input.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out.write_unaligned(read(&*input)) };
        TypeBridgeStatus::Ok
    })
}

unsafe fn owned_identity<Input>(
    input: *const Input,
    out: *mut *mut TypeBridgeMigrationIdentity,
    read: impl FnOnce(&Input) -> MigrationIdentitySnapshot,
) -> TypeBridgeStatus {
    guarded(|| {
        if input.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out.write_unaligned(ptr::null_mut()) };
        let state = read(unsafe { &*input });
        unsafe {
            out.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

unsafe fn identity_view(
    identity: *const TypeBridgeMigrationIdentity,
    out: *mut TypeBridgeByteView,
    read: impl FnOnce(&TypeBridgeMigrationIdentity) -> &[u8],
) -> TypeBridgeStatus {
    guarded(|| {
        if identity.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let bytes = read(unsafe { &*identity });
        unsafe {
            out.write_unaligned(TypeBridgeByteView {
                data: bytes.as_ptr(),
                length: bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}
