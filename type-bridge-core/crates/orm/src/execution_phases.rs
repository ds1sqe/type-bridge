//! Invocation-local execution diagnostics shared by the ORM and server.
//! Disabled collection retains only scalar state and performs no phase clock reads.

use std::cell::RefCell;
use std::future::Future;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use type_bridge_contract::execution_timeline::{
    ExecutionPhase, ExecutionPhaseStamp, ExecutionTimeline,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticDetailValue, SdkDiagnosticName, SdkExecutionDiagnostic, SdkQueryDiagnosticIdentity,
};

tokio::task_local! {
    static PHASES: RefCell<Option<ExecutionPhases>>;
}

/// Captured phase state for a single invocation clock.
#[derive(Clone)]
pub struct ExecutionPhases {
    started: Instant,
    current: ExecutionPhase,
    statements: u32,
    finished: bool,
    timeline: Option<(u64, Vec<ExecutionPhaseStamp>)>,
}

impl ExecutionPhases {
    fn new(started: Instant, collect: bool) -> Self {
        let mut phases = Self {
            started,
            current: ExecutionPhase::Fired,
            statements: 0,
            finished: false,
            timeline: None,
        };
        if collect || tracing::enabled!(target: "type_bridge.plan", tracing::Level::DEBUG) {
            phases.enable_collection();
        }
        phases
    }

    fn enable_collection(&mut self) {
        if self.timeline.is_some() {
            return;
        }
        // A proxy learns the opt-in flag during decode; admission and fired share
        // its receive boundary, so no intermediate clock samples are needed.
        let elapsed = tokio::time::Instant::now()
            .into_std()
            .saturating_duration_since(self.started);
        let anchor = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.saturating_sub(elapsed).as_millis()).unwrap_or(u64::MAX)
            });
        let mut stamps = vec![ExecutionPhaseStamp {
            phase: ExecutionPhase::Fired,
            offset_milliseconds: 0,
            statement_index: None,
            failing_phase: None,
        }];
        tracing::debug!(target: "type_bridge.plan", phase = "fired", offset_milliseconds = 0_u64, anchor_unix_milliseconds = anchor, "type_bridge.plan.phase");
        if self.current == ExecutionPhase::Accepted {
            stamps.push(ExecutionPhaseStamp {
                phase: ExecutionPhase::Accepted,
                offset_milliseconds: 0,
                statement_index: None,
                failing_phase: None,
            });
            tracing::debug!(target: "type_bridge.plan", phase = "accepted", offset_milliseconds = 0_u64, anchor_unix_milliseconds = anchor, "type_bridge.plan.phase");
        }
        self.timeline = Some((anchor, stamps));
    }

    /// Record a lifecycle boundary. Nested execution cannot restart earlier phases.
    pub fn record(&mut self, phase: ExecutionPhase) {
        if self.finished
            || (phase == ExecutionPhase::Accepted && self.current != ExecutionPhase::Fired)
            || (phase == ExecutionPhase::ExecutionStarted
                && self.current != ExecutionPhase::Accepted)
        {
            return;
        }

        if phase != ExecutionPhase::DbWrite && phase == self.current {
            return;
        }
        let index = (phase == ExecutionPhase::DbWrite).then_some(self.statements);
        if index.is_some() {
            self.statements = self.statements.saturating_add(1);
        }
        if let Some((anchor, stamps)) = &mut self.timeline {
            let offset = u64::try_from(
                tokio::time::Instant::now()
                    .into_std()
                    .saturating_duration_since(self.started)
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            let failing_phase = (phase == ExecutionPhase::Failed).then_some(self.current);
            stamps.push(ExecutionPhaseStamp {
                phase,
                offset_milliseconds: offset,
                statement_index: index,
                failing_phase,
            });
            tracing::debug!(target: "type_bridge.plan", phase = phase.as_str(), offset_milliseconds = offset, anchor_unix_milliseconds = *anchor, statement_index = index, failing_phase = failing_phase.map(ExecutionPhase::as_str), "type_bridge.plan.phase");
        }
        if phase == ExecutionPhase::Failed {
            self.finished = true;
        } else {
            self.current = phase;
            self.finished = phase == ExecutionPhase::Committed;
        }
    }

    pub(crate) fn orm_error(&self, error: crate::OrmError) -> crate::OrmError {
        let diagnostic = crate::execution_diagnostic::lower_orm_error(
            &error,
            type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
        );
        let decorated = self.diagnostic(diagnostic.clone());
        if decorated == diagnostic {
            error
        } else {
            crate::OrmError::ExecutionDiagnostic(Box::new(decorated))
        }
    }

    /// Attach phase and elapsed facts to deadline or cancellation diagnostics.
    pub fn diagnostic(&self, mut diagnostic: SdkExecutionDiagnostic) -> SdkExecutionDiagnostic {
        if diagnostic.code().as_str() != "transaction_deadline_exceeded"
            && diagnostic.category()
                != type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::Cancelled
        {
            return diagnostic;
        }
        // A signed server timeline keeps its own clock; never overwrite it with client offsets.
        if diagnostic
            .details()
            .keys()
            .any(|key| key.as_str() == "execution_phase")
        {
            return diagnostic;
        }
        let elapsed = u64::try_from(
            tokio::time::Instant::now()
                .into_std()
                .saturating_duration_since(self.started)
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let mut details = vec![
            (
                "execution_phase",
                SdkDiagnosticDetailValue::QueryIdentity(
                    SdkQueryDiagnosticIdentity::new(self.current.as_str()).expect("static phase"),
                ),
            ),
            (
                "elapsed_milliseconds",
                SdkDiagnosticDetailValue::Count(elapsed),
            ),
        ];
        if let Some((anchor, stamps)) = &self.timeline {
            let mut stamps = stamps.clone();
            if !self.finished {
                stamps.push(ExecutionPhaseStamp {
                    phase: ExecutionPhase::Failed,
                    offset_milliseconds: elapsed,
                    statement_index: None,
                    failing_phase: Some(self.current),
                });
            }
            let timeline = ExecutionTimeline::new(*anchor, stamps)
                .expect("execution phases follow the lifecycle");
            details.push((
                "execution_timeline",
                SdkDiagnosticDetailValue::ExecutionTimeline(timeline),
            ));
        }
        for (name, detail) in details {
            if !diagnostic.details().keys().any(|key| key.as_str() == name) {
                diagnostic = diagnostic
                    .try_with_detail(SdkDiagnosticName::new(name).expect("static detail"), detail)
                    .expect("phase details fit the diagnostic budget");
            }
        }
        diagnostic
    }
}

/// Reuse the outer invocation across staged programs and borrowed transactions.
pub async fn observe<T>(
    started: Instant,
    collect: bool,
    future: impl Future<Output = T>,
) -> (T, Option<ExecutionPhases>) {
    tokio::pin!(future);
    if PHASES
        .try_with(|cell| cell.borrow().is_some())
        .unwrap_or(false)
    {
        return (future.await, None);
    }
    PHASES
        .scope(
            RefCell::new(Some(ExecutionPhases::new(started, collect))),
            async {
                let result = future.await;
                let phases = PHASES.with(|cell| cell.borrow_mut().take());
                (result, phases)
            },
        )
        .await
}

/// Observe a result-bearing call and close its timeline once, at the outer invocation.
pub async fn observe_result<T, E>(
    started: Instant,
    collect: bool,
    future: impl Future<Output = Result<T, E>>,
) -> (Result<T, E>, Option<ExecutionPhases>) {
    tokio::pin!(future);
    if PHASES
        .try_with(|cell| cell.borrow().is_some())
        .unwrap_or(false)
    {
        return (future.await, None);
    }
    observe(started, collect, async {
        let result = future.await;
        finish(result.is_ok());
        result
    })
    .await
}

/// Record a phase on the active invocation, if any.
pub fn record(phase: ExecutionPhase) {
    let _ = PHASES.try_with(|cell| {
        if let Some(phases) = cell.borrow_mut().as_mut() {
            phases.record(phase);
        }
    });
}

pub(crate) fn started() -> Option<Instant> {
    PHASES
        .try_with(|cell| cell.borrow().as_ref().map(|phases| phases.started))
        .ok()
        .flatten()
}

/// Preserve a signed server clock and add facts from the active local invocation.
pub fn annotate(diagnostic: SdkExecutionDiagnostic) -> SdkExecutionDiagnostic {
    PHASES
        .try_with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|phases| phases.diagnostic(diagnostic.clone()))
        })
        .ok()
        .flatten()
        .unwrap_or(diagnostic)
}

/// Enable collection after decoding a proxy request's opt-in flag.
/// Call during admission, before execution begins.
pub fn enable_collection() {
    let _ = PHASES.try_with(|cell| {
        if let Some(phases) = cell.borrow_mut().as_mut() {
            phases.enable_collection();
        }
    });
}

/// Record that the server accepted the call at its receive boundary.
pub fn received() {
    let _ = PHASES.try_with(|cell| {
        if let Some(phases) = cell.borrow_mut().as_mut() {
            phases.current = ExecutionPhase::Accepted;
            if let Some((anchor, stamps)) = &mut phases.timeline {
                stamps.push(ExecutionPhaseStamp { phase: ExecutionPhase::Accepted, offset_milliseconds: 0, statement_index: None, failing_phase: None });
                tracing::debug!(target: "type_bridge.plan", phase = "accepted", offset_milliseconds = 0_u64, anchor_unix_milliseconds = *anchor, "type_bridge.plan.phase");
            }
        }
    });
}

/// Capture the current clock and records before spawning owned execution work.
pub fn snapshot() -> Option<ExecutionPhases> {
    PHASES.try_with(|cell| cell.borrow().clone()).ok().flatten()
}

/// Continue a captured invocation on a detached task without restarting its clock.
pub async fn continue_with<T>(
    phases: Option<ExecutionPhases>,
    future: impl Future<Output = T>,
) -> T {
    PHASES.scope(RefCell::new(phases), future).await
}

/// Finish the active timeline without replacing its failing phase identity.
pub fn finish(success: bool) {
    record(if success {
        ExecutionPhase::Committed
    } else {
        ExecutionPhase::Failed
    });
}

pub(crate) fn collecting() -> bool {
    PHASES
        .try_with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|phases| phases.timeline.is_some())
        })
        .unwrap_or(false)
}

pub(crate) fn dispatching() {
    let _ = PHASES.try_with(|cell| {
        if let Some(phases) = cell.borrow_mut().as_mut()
            && phases.timeline.is_none()
        {
            phases.current = ExecutionPhase::DbWrite;
        }
    });
}

pub(crate) fn dispatched() {
    record(ExecutionPhase::DbWrite);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deadline_error() -> SdkExecutionDiagnostic {
        SdkExecutionDiagnostic::resource_limit(
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticCode::new(
                "transaction_deadline_exceeded",
            )
            .unwrap(),
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticMessage::new(
                "Execution deadline elapsed",
            )
            .unwrap(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_collection_keeps_phase_and_elapsed_without_a_timeline() {
        let (_, phases) = observe(tokio::time::Instant::now().into_std(), false, async {
            record(ExecutionPhase::Accepted);
            record(ExecutionPhase::ExecutionStarted);
            record(ExecutionPhase::DbWrite);
            tokio::time::advance(std::time::Duration::from_millis(7)).await;
        })
        .await;
        let phases = phases.unwrap();
        assert!(phases.timeline.is_none());
        let diagnostic =
            serde_json::to_value(phases.diagnostic(deadline_error()).host_json()).unwrap();
        assert_eq!(
            diagnostic["details"]["execution_phase"]["value"],
            "db_write"
        );
        assert_eq!(diagnostic["details"]["elapsed_milliseconds"]["value"], "7");
        assert!(diagnostic["details"].get("execution_timeline").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn collection_preserves_outer_clock_and_statement_indices() {
        let (_, phases) = observe(tokio::time::Instant::now().into_std(), true, async {
            record(ExecutionPhase::Accepted);
            record(ExecutionPhase::ExecutionStarted);
            record(ExecutionPhase::DbWrite);
            tokio::time::advance(std::time::Duration::from_millis(4)).await;
            let (_, nested) = observe(tokio::time::Instant::now().into_std(), false, async {
                record(ExecutionPhase::DbWrite);
            })
            .await;
            assert!(nested.is_none());
        })
        .await;
        let diagnostic =
            serde_json::to_value(phases.unwrap().diagnostic(deadline_error()).host_json()).unwrap();
        let timeline = &diagnostic["details"]["execution_timeline"]["value"];
        assert!(timeline["anchor_unix_milliseconds"].as_u64().unwrap() > 0);
        assert_eq!(timeline["stamps"][3]["statement_index"], 0);
        assert_eq!(timeline["stamps"][4]["statement_index"], 1);
        assert_eq!(timeline["stamps"][4]["offset_milliseconds"], 4);
        assert_eq!(timeline["stamps"][5]["phase"], "failed");
        assert_eq!(timeline["stamps"][5]["failing_phase"], "db_write");
    }
}
