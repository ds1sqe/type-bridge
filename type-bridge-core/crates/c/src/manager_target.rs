//! Borrowed execution targets shared by canonical C manager entry points.

use std::future::Future;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::{
    AnswerCancellation, AnswerTransport, ProjectedBatchInvocationControl,
    ProjectedManagerFilterInvocationControl, QueryTarget,
};

use crate::abi::{SchemaPackageState, TypeBridgeStatus};
use crate::generated_preflight::{DirectOutputPreflight, GENERATED_INPUT_CANCELLATION};
use crate::policy::parse_common_limits;
use crate::query::{TypeBridgeProxyConnection, TypeBridgeQueryExecutionLimitsV1};
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction, check_database_borrowed_ranges,
    check_read_transaction_borrowed_ranges, check_write_transaction_borrowed_ranges,
    poisoned_transaction_diagnostic,
};

#[derive(Clone, Copy)]
pub(crate) enum RawTarget {
    Database(*const TypeBridgeDatabase),
    Read(*const TypeBridgeReadTransaction),
    Write(*const TypeBridgeWriteTransaction),
    Proxy(*const TypeBridgeProxyConnection),
}

#[derive(Clone, Copy)]
pub(crate) enum Target<'a> {
    Database(&'a TypeBridgeDatabase),
    Read(&'a TypeBridgeReadTransaction),
    Write(&'a TypeBridgeWriteTransaction),
    Proxy(&'a TypeBridgeProxyConnection),
}

impl RawTarget {
    pub(crate) unsafe fn preflight(
        self,
        outputs: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        macro_rules! check {
            ($pointer:expr, $kind:ty, $deep:expr) => {{
                let pointer = $pointer;
                if !pointer.is_null() {
                    outputs.check_bytes(pointer.cast(), size_of::<$kind>())?;
                    // SAFETY: the caller retains the complete target for this call.
                    $deep(outputs, unsafe { &*pointer })?;
                }
            }};
        }
        match self {
            Self::Database(value) => {
                check!(value, TypeBridgeDatabase, check_database_borrowed_ranges)
            }
            Self::Read(value) => check!(
                value,
                TypeBridgeReadTransaction,
                check_read_transaction_borrowed_ranges
            ),
            Self::Write(value) => check!(
                value,
                TypeBridgeWriteTransaction,
                check_write_transaction_borrowed_ranges
            ),
            Self::Proxy(value) => check!(
                value,
                TypeBridgeProxyConnection,
                |outputs, value: &TypeBridgeProxyConnection| value.check_borrowed_ranges(outputs)
            ),
        }
        Ok(())
    }

    pub(crate) unsafe fn borrow<'a>(self) -> Option<Target<'a>> {
        // SAFETY: caller preflight fenced the complete target and retains its lifetime.
        unsafe {
            match self {
                Self::Database(value) => value.as_ref().map(Target::Database),
                Self::Read(value) => value.as_ref().map(Target::Read),
                Self::Write(value) => value.as_ref().map(Target::Write),
                Self::Proxy(value) => value.as_ref().map(Target::Proxy),
            }
        }
    }
}

impl<'a> Target<'a> {
    pub(crate) fn package(self) -> &'a Arc<SchemaPackageState> {
        match self {
            Self::Database(value) => value.package_state(),
            Self::Read(value) => value.package_state(),
            Self::Write(value) => value.package_state(),
            Self::Proxy(value) => value.package_state(),
        }
    }

    pub(crate) fn query_target(self) -> Result<QueryTarget<'a>, SdkExecutionDiagnostic> {
        let inactive = || {
            SdkExecutionDiagnostic::invalid_input(
                SdkDiagnosticCode::new("c_manager_transaction_inactive").unwrap(),
                SdkDiagnosticMessage::new("The manager transaction is no longer active").unwrap(),
            )
        };
        match self {
            Self::Database(value) => Ok(QueryTarget::Database(value.orm_database())),
            Self::Proxy(value) => Ok(value.manager_target()),
            Self::Read(value) => {
                if value.is_poisoned() {
                    return Err(poisoned_transaction_diagnostic());
                }
                value
                    .context()
                    .map(QueryTarget::Transaction)
                    .ok_or_else(inactive)
            }
            Self::Write(value) => {
                if value.is_poisoned() {
                    return Err(poisoned_transaction_diagnostic());
                }
                value
                    .context()
                    .map(QueryTarget::Transaction)
                    .ok_or_else(inactive)
            }
        }
    }

    pub(crate) fn block_on<F: Future>(self, future: F) -> F::Output {
        let result = catch_unwind(AssertUnwindSafe(|| match self {
            Self::Database(value) => value.block_on(future),
            Self::Read(value) => value.block_on(future),
            Self::Write(value) => value.block_on(future),
            Self::Proxy(value) => value.manager_block_on(future),
        }));
        match result {
            Ok(value) => value,
            Err(panic) => {
                match self {
                    Self::Read(value) => value.mark_poisoned(),
                    Self::Write(value) => value.mark_poisoned(),
                    _ => {}
                }
                resume_unwind(panic)
            }
        }
    }

    pub(crate) unsafe fn control(
        self,
        limits: *const TypeBridgeQueryExecutionLimitsV1,
        cancellation: *const TypeBridgeCancellation,
    ) -> Result<ProjectedManagerFilterInvocationControl, SdkExecutionDiagnostic> {
        // SAFETY: caller retains the preflighted optional policy owners.
        let control = unsafe { self.batch_control(limits, cancellation) }?;
        let (limits, deadline, cancellation) = control.into_parts();
        Ok(ProjectedManagerFilterInvocationControl::from_parts(
            limits,
            deadline,
            cancellation,
        ))
    }

    pub(crate) unsafe fn batch_control(
        self,
        limits: *const TypeBridgeQueryExecutionLimitsV1,
        cancellation: *const TypeBridgeCancellation,
    ) -> Result<ProjectedBatchInvocationControl, SdkExecutionDiagnostic> {
        let transport = if matches!(self, Self::Proxy(_)) {
            AnswerTransport::Proxy
        } else {
            AnswerTransport::Direct
        };
        // SAFETY: caller preflight fenced optional descriptors and cancellation owner.
        let limits = if limits.is_null() {
            None
        } else {
            Some(unsafe { limits.read_unaligned() })
        };
        let limits = parse_common_limits(limits, transport)?;
        let cancellation = if cancellation.is_null() {
            AnswerCancellation::default()
        } else {
            unsafe { &*cancellation }.answer_cancellation()
        };
        Ok(ProjectedBatchInvocationControl::capture(
            limits,
            cancellation,
        ))
    }
}

pub(crate) fn policy_preflight(
    outputs: &DirectOutputPreflight,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
) -> Result<(), TypeBridgeStatus> {
    if !limits.is_null() {
        outputs.check_bytes(limits.cast(), size_of::<TypeBridgeQueryExecutionLimitsV1>())?;
    }
    outputs.check_object_kind(GENERATED_INPUT_CANCELLATION, cancellation.cast())
}
