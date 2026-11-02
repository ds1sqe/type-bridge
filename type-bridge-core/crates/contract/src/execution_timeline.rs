//! Diagnostic-only phase timestamps for one execution clock.

use serde::{Deserialize, Serialize};

/// Named execution boundary; `DbWrite` means a statement was sent, including reads.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    /// The caller began execution.
    Fired,
    /// The transaction opened or the server received the request.
    Accepted,
    /// Statement execution began.
    ExecutionStarted,
    /// One statement was sent to the database.
    DbWrite,
    /// The final deadline check before dispatching commit.
    BeforeCommit,
    /// Commit completed.
    Committed,
    /// Execution failed in the named failing phase.
    Failed,
}

impl ExecutionPhase {
    /// Stable diagnostic and tracing spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Accepted => "accepted",
            Self::ExecutionStarted => "execution_started",
            Self::DbWrite => "db_write",
            Self::BeforeCommit => "before_commit",
            Self::Committed => "committed",
            Self::Failed => "failed",
        }
    }
}

/// One offset on the timeline's monotonic clock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPhaseStamp {
    /// Boundary being recorded.
    pub phase: ExecutionPhase,
    /// Milliseconds from execution entry, never a wall-clock difference.
    pub offset_milliseconds: u64,
    /// Zero-based provider statement index, present only for `db_write`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub statement_index: Option<u32>,
    /// Boundary in progress when execution failed, present only for `failed`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failing_phase: Option<ExecutionPhase>,
}

/// One UTC anchor and ordered offsets from a single process clock.
///
/// Client and server timelines must remain separate: their clocks differ.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TimelineWire")]
pub struct ExecutionTimeline {
    anchor_unix_milliseconds: u64,
    stamps: Vec<ExecutionPhaseStamp>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineWire {
    anchor_unix_milliseconds: u64,
    stamps: Vec<ExecutionPhaseStamp>,
}

impl TryFrom<TimelineWire> for ExecutionTimeline {
    type Error = &'static str;
    fn try_from(value: TimelineWire) -> Result<Self, Self::Error> {
        Self::new(value.anchor_unix_milliseconds, value.stamps)
    }
}

impl ExecutionTimeline {
    /// Admit a complete, ordered timeline using the existing statement ceiling.
    pub fn new(
        anchor_unix_milliseconds: u64,
        stamps: Vec<ExecutionPhaseStamp>,
    ) -> Result<Self, &'static str> {
        let value = Self {
            anchor_unix_milliseconds,
            stamps,
        };
        value.validate(crate::limits::MAX_STAGED_STATEMENTS_V3.saturating_mul(2))?;
        Ok(value)
    }

    /// UTC time captured once at this process's execution entry.
    pub const fn anchor_unix_milliseconds(&self) -> u64 {
        self.anchor_unix_milliseconds
    }

    /// Ordered phase records; borrows the owning diagnostic.
    pub fn stamps(&self) -> &[ExecutionPhaseStamp] {
        &self.stamps
    }

    /// Enforce the call's existing statement allowance, without a separate limit.
    pub fn validate(&self, statement_limit: u32) -> Result<(), &'static str> {
        let mut state = TimelineState::new(statement_limit);
        for stamp in &self.stamps {
            state.accept(stamp)?;
        }
        state.finish()
    }
}

struct TimelineState {
    limit: u32,
    count: usize,
    statements: u32,
    previous: ExecutionPhase,
    offset: u64,
    over_limit: bool,
}

impl TimelineState {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            count: 0,
            statements: 0,
            previous: ExecutionPhase::Fired,
            offset: 0,
            over_limit: false,
        }
    }
    fn accept(&mut self, stamp: &ExecutionPhaseStamp) -> Result<(), &'static str> {
        use ExecutionPhase::*;
        let invalid = "invalid execution timeline";
        if self.count >= self.limit as usize + 6
            || (stamp.phase == DbWrite && self.statements >= self.limit)
        {
            self.over_limit = true;
            return Err("execution timeline exceeds the statement bound");
        }
        if stamp.offset_milliseconds < self.offset
            || (self.count == 0 && (stamp.phase != Fired || stamp.offset_milliseconds != 0))
        {
            return Err(invalid);
        }
        if self.count != 0
            && !matches!(
                (self.previous, stamp.phase),
                (Fired, Accepted | Failed)
                    | (Accepted, ExecutionStarted | Failed)
                    | (ExecutionStarted, DbWrite | BeforeCommit | Failed)
                    | (DbWrite, DbWrite | BeforeCommit | Failed)
                    | (BeforeCommit, Committed | Failed)
            )
        {
            return Err(invalid);
        }
        if stamp.phase == DbWrite {
            if stamp.statement_index != Some(self.statements) {
                return Err(invalid);
            }
            self.statements += 1;
        } else if stamp.statement_index.is_some() {
            return Err(invalid);
        }
        if stamp.phase == Failed {
            if stamp.failing_phase != Some(self.previous) {
                return Err(invalid);
            }
        } else if stamp.failing_phase.is_some() {
            return Err(invalid);
        }
        self.previous = stamp.phase;
        self.offset = stamp.offset_milliseconds;
        self.count += 1;
        Ok(())
    }
    fn finish(&self) -> Result<(), &'static str> {
        if self.count >= 2
            && matches!(
                self.previous,
                ExecutionPhase::Committed | ExecutionPhase::Failed
            )
        {
            Ok(())
        } else {
            Err("invalid execution timeline")
        }
    }
}

/// Borrow framing and visit fixed records before allocating an owned timeline.
pub(crate) fn preflight_json(
    json: &str,
    statement_limit: u32,
) -> Result<(), crate::diagnostic::Diagnostic> {
    use serde::de::{DeserializeSeed, SeqAccess, Visitor};
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Borrowed<'a> {
        anchor_unix_milliseconds: u64,
        #[serde(borrow)]
        stamps: &'a serde_json::value::RawValue,
    }
    struct Records<'a>(&'a mut TimelineState);
    impl<'de> Visitor<'de> for Records<'_> {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bounded execution phase records")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while let Some(stamp) = seq.next_element::<ExecutionPhaseStamp>()? {
                self.0.accept(&stamp).map_err(serde::de::Error::custom)?;
            }
            self.0.finish().map_err(serde::de::Error::custom)
        }
    }
    impl<'de> DeserializeSeed<'de> for Records<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            deserializer.deserialize_seq(self)
        }
    }
    let invalid = || {
        crate::diagnostic::Diagnostic::new(
            crate::diagnostic::DiagnosticCategory::InvalidContract,
            crate::diagnostic::DiagnosticCode::new("query_remote_v3_execution_timeline")
                .expect("static code"),
            "Execution timeline has invalid phase records",
        )
    };
    let borrowed: Borrowed<'_> = serde_json::from_str(json).map_err(|_| invalid())?;
    let _anchor = borrowed.anchor_unix_milliseconds;
    let mut state = TimelineState::new(statement_limit);
    let mut deserializer = serde_json::Deserializer::from_str(borrowed.stamps.get());
    if Records(&mut state).deserialize(&mut deserializer).is_err() {
        if state.over_limit {
            return Err(crate::diagnostic::Diagnostic::new(
                crate::diagnostic::DiagnosticCategory::ResourceLimit,
                crate::diagnostic::DiagnosticCode::new("query_remote_v3_execution_timeline_limit")
                    .expect("static code"),
                "Execution timeline exceeds the request statement bound",
            ));
        }
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_round_trip_and_statement_budget() {
        let mut stamps = vec![ExecutionPhaseStamp {
            phase: ExecutionPhase::Fired,
            offset_milliseconds: 0,
            statement_index: None,
            failing_phase: None,
        }];
        for phase in [ExecutionPhase::Accepted, ExecutionPhase::ExecutionStarted] {
            stamps.push(ExecutionPhaseStamp {
                phase,
                offset_milliseconds: 1,
                statement_index: None,
                failing_phase: None,
            });
        }
        for index in 0..40 {
            stamps.push(ExecutionPhaseStamp {
                phase: ExecutionPhase::DbWrite,
                offset_milliseconds: 2 + u64::from(index),
                statement_index: Some(index),
                failing_phase: None,
            });
        }
        stamps.push(ExecutionPhaseStamp {
            phase: ExecutionPhase::Failed,
            offset_milliseconds: 42,
            statement_index: None,
            failing_phase: Some(ExecutionPhase::DbWrite),
        });
        let timeline = ExecutionTimeline::new(1000, stamps).unwrap();
        assert!(timeline.validate(40).is_ok());
        assert!(timeline.validate(39).is_err());
        let json = serde_json::to_value(&timeline).unwrap();
        assert_eq!(
            serde_json::from_value::<ExecutionTimeline>(json.clone()).unwrap(),
            timeline
        );
        for field in ["offset_milliseconds", "statement_index"] {
            let mut invalid = json.clone();
            invalid["stamps"][4][field] = serde_json::json!(0);
            assert!(serde_json::from_value::<ExecutionTimeline>(invalid).is_err());
        }
        let mut invalid = json;
        invalid["stamps"][43]["failing_phase"] = serde_json::json!("fired");
        assert!(serde_json::from_value::<ExecutionTimeline>(invalid).is_err());
    }
}
