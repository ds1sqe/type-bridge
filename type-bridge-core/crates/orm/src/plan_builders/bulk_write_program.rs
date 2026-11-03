//! Fixed-limit pieces of one manager write, all executed by its existing lease.
use super::*;
use crate::query_plan_exec::PlanAnswer;
use std::collections::VecDeque;
use type_bridge_contract::query_remote::RemoteValue;
use type_bridge_contract::query_remote_v3::StagedAnswerV3;

#[path = "bulk_write_match.rs"]
mod match_put;
pub(super) use match_put::wide_match;

#[path = "bulk_write_references.rs"]
mod references;
pub(super) use references::resolve_references;

#[path = "bulk_write_static.rs"]
mod static_stack;
use static_stack::StaticProgram;

pub(crate) type ReferenceSnapshots = BTreeMap<usize, BTreeMap<RoleId, Vec<String>>>;

pub(crate) type IdentityLayout = Vec<Option<Vec<(usize, String)>>>;

pub(crate) enum ReferenceContext {
    Guard(references::GuardContext),
    Snapshot(usize),
}

pub(crate) enum WriteProgram {
    Compact(Vec<BuiltPlan>),
    Sequential(Vec<(BuiltPlan, Vec<usize>)>),
    Split(SplitProgram),
    Static(Box<StaticProgram>),
    Chain(VecDeque<WriteProgram>),
    MatchWide(Box<match_put::MatchProgram>),
    References(Box<references::ReferenceProgram>),
}
impl WriteProgram {
    pub(crate) fn compact(plans: Vec<BuiltPlan>) -> Self {
        Self::Compact(plans)
    }
    /// Freeze allocation dependencies before opening the manager transaction.
    pub(crate) fn prepare_static(self) -> Result<Self, Diagnostic> {
        match self {
            Self::Split(program)
                if program
                    .steps
                    .iter()
                    .any(|step| matches!(step, Step::Target { .. }))
                    && program.steps.iter().all(|step| match step {
                        Step::Ready(step) => {
                            !step.candidate_filter
                                && step.reference_resolution.as_ref().is_none_or(|resolution| {
                                    step.reference_guard && resolution.guard.is_some()
                                })
                        }
                        Step::Target { .. } => true,
                    }) =>
            {
                Ok(Self::Static(Box::new(StaticProgram::new(program)?)))
            }
            Self::Chain(mut programs) => {
                for _ in 0..programs.len() {
                    let program = programs.pop_front().expect("one existing program");
                    programs.push_back(program.prepare_static()?);
                }
                Ok(Self::Chain(programs))
            }
            Self::References(mut program) => {
                program.prepare_static()?;
                Ok(Self::References(program))
            }
            program => Ok(program),
        }
    }

    pub(crate) fn records_answers(&self) -> bool {
        match self {
            Self::Static(program) => program.records_answers(),
            Self::MatchWide(program) => program.records_answers(),
            Self::References(program) => program.records_answers(),
            Self::Chain(programs) => programs.front().is_some_and(Self::records_answers),
            _ => false,
        }
    }

    pub(crate) fn record_answer(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::Static(program) => program.record_answer(step, answer)?,
            Self::MatchWide(program) => program.record_answer(step, answer)?,
            Self::References(program) => program.record_answer(step, answer)?,
            Self::Chain(programs) => programs
                .front_mut()
                .ok_or_else(identity_error)?
                .record_answer(step, answer)?,
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn statement_count(&self) -> usize {
        match self {
            Self::Compact(plans) => plans.len(),
            Self::Sequential(plans) => plans.len(),
            Self::Split(program) => program.steps.len(),
            Self::Static(program) => program.statement_count(),
            Self::Chain(programs) => programs.iter().map(Self::statement_count).sum(),
            Self::MatchWide(program) => program.statement_count(),
            Self::References(program) => program.statement_count(),
        }
    }
    pub(crate) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        match self {
            Self::Compact(plans) => Ok((!plans.is_empty()).then(|| WriteStep {
                plan: plans.remove(0),
                complete: true,
                ordinal: 0,
                expected_iid: None,
                missing_iid_noop: false,
                candidate_filter: false,
                reference_guard: false,
                reference_resolution: None,
            })),
            Self::Sequential(plans) => Ok((!plans.is_empty()).then(|| WriteStep {
                plan: plans.remove(0).0,
                complete: true,
                ordinal: 0,
                expected_iid: None,
                missing_iid_noop: false,
                candidate_filter: false,
                reference_guard: false,
                reference_resolution: None,
            })),
            Self::Split(program) => program.next(),
            Self::Static(program) => program.next(),
            Self::MatchWide(program) => program.next(),
            Self::References(program) => program.next(),
            Self::Chain(programs) => {
                while let Some(program) = programs.front_mut() {
                    if let Some(step) = program.next()? {
                        return Ok(Some(step));
                    }
                    programs.pop_front();
                }
                Ok(None)
            }
        }
    }
    pub(crate) fn guard_contexts(&self) -> BTreeMap<(usize, u16), ReferenceContext> {
        match self {
            Self::Static(program) => program.guard_contexts(),
            Self::References(program) => program.guard_contexts(),
            Self::Chain(programs) => {
                let mut contexts = BTreeMap::new();
                let mut offset = 0;
                for program in programs {
                    contexts.extend(
                        program
                            .guard_contexts()
                            .into_iter()
                            .map(|((index, column), context)| ((index + offset, column), context)),
                    );
                    offset += program.statement_count();
                }
                contexts
            }
            _ => BTreeMap::new(),
        }
    }

    pub(crate) fn staged_entries(
        &self,
        authority: &PlanAuthority,
    ) -> Result<Option<Vec<type_bridge_contract::query_remote_v3::StagedPlanV3>>, Diagnostic> {
        use type_bridge_contract::query_remote_v3::StagedPlanV3;
        let entry = |plan: &BuiltPlan| {
            StagedPlanV3::new(
                plan.local(),
                plan.local().given_rows().unwrap_or_default().to_vec(),
                authority.declared(),
            )
        };
        match self {
            Self::Compact(plans) => plans.iter().map(entry).collect::<Result<Vec<_>, _>>().map(Some),
            Self::Sequential(plans) => plans.iter().map(|(plan, _)| entry(plan)).collect::<Result<Vec<_>, _>>().map(Some),
            Self::Static(program) => Ok(Some(program.staged_entries())),
            Self::Chain(programs) => {
                let mut entries = Vec::with_capacity(programs.iter().map(Self::statement_count).sum());
                for program in programs {
                    let Some(group) = program.staged_entries(authority)? else { return Ok(None) };
                    let offset = u32::try_from(entries.len()).map_err(|_| identity_error())?;
                    for entry in group { entries.push(entry.rebase_result_refs(offset)?); }
                }
                Ok(Some(entries))
            }
            Self::Split(program) if program.steps.iter().all(|step| matches!(step, Step::Ready(step) if !step.reference_guard && step.reference_resolution.is_none() && !step.candidate_filter)) => {
                program.steps.iter().map(|step| match step {
                    Step::Ready(step) => entry(&step.plan),
                    _ => unreachable!("fixed ready steps"),
                }).collect::<Result<Vec<_>, _>>().map(Some)
            }
            Self::References(program) => program.staged_entries(authority),
            Self::Split(_) | Self::MatchWide(_) => Ok(None),
        }
    }

    pub(crate) fn into_static(self) -> Result<Vec<BuiltPlan>, Diagnostic> {
        match self {
            Self::Compact(plans) => Ok(plans),
            Self::MatchWide(_) | Self::References(_) | Self::Static(_) => Err(identity_error()),
            Self::Sequential(plans) => Ok(plans.into_iter().map(|(plan, _)| plan).collect()),
            Self::Chain(programs) => programs
                .into_iter()
                .map(Self::into_static)
                .collect::<Result<Vec<_>, _>>()
                .map(|plans| plans.into_iter().flatten().collect()),
            Self::Split(program) => program
                .steps
                .into_iter()
                .map(|step| match step {
                    Step::Ready(step) => Ok(step.plan),
                    Step::Target { .. } => Err(identity_error()),
                })
                .collect(),
        }
    }

    pub(crate) fn identity_layout(&self) -> Option<IdentityLayout> {
        match self {
            Self::Sequential(plans) => Some(
                plans
                    .iter()
                    .map(|(plan, ordinals)| {
                        let QueryOutput::Rows { columns } = plan.local().output() else {
                            unreachable!("sequential puts return row identities")
                        };
                        Some(
                            ordinals
                                .iter()
                                .zip(columns)
                                .map(|(ordinal, column)| {
                                    (
                                        *ordinal,
                                        plan.local().bindings()[usize::from(column.get())]
                                            .variable()
                                            .as_str()
                                            .to_owned(),
                                    )
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            Self::Chain(programs) => Some(
                programs
                    .iter()
                    .flat_map(|program| {
                        program.identity_layout().unwrap_or_else(|| match program {
                            Self::Compact(plans) => vec![None; plans.len()],
                            Self::MatchWide(_) => vec![None],
                            Self::Static(program) => vec![None; program.completion_count()],
                            Self::References(_) => vec![None],
                            Self::Split(split) => split
                                .steps
                                .iter()
                                .filter_map(|step| match step {
                                    Step::Ready(step) if step.complete => Some(None),
                                    Step::Target { complete: true, .. } => Some(None),
                                    _ => None,
                                })
                                .collect(),
                            _ => unreachable!("nested sequences carry their layouts"),
                        })
                    })
                    .collect(),
            ),
            Self::Compact(_)
            | Self::Split(_)
            | Self::Static(_)
            | Self::MatchWide(_)
            | Self::References(_) => None,
        }
    }

    pub(crate) fn accept_reference_iids(
        &mut self,
        step: &WriteStep,
        iids: Vec<(usize, String)>,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::References(program) => program.accept(step, iids),
            _ => Err(identity_error()),
        }
    }
    pub(crate) fn take_reference_snapshots(&mut self) -> Option<Box<ReferenceSnapshots>> {
        match self {
            Self::References(program) => program.take_snapshots(),
            _ => None,
        }
    }

    pub(crate) fn accept_candidates(
        &mut self,
        step: &WriteStep,
        answer: &PlanAnswer,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::MatchWide(program) => program.accept_candidates(step, answer),
            Self::Chain(programs) => programs
                .front_mut()
                .ok_or_else(identity_error)?
                .accept_candidates(step, answer),
            _ => Err(identity_error()),
        }
    }

    pub(crate) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: Option<String>,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::Split(program) => program.accept_identity(step, iid),
            Self::Static(_) => match iid {
                Some(iid) => step.check_identity(&iid),
                None if step.missing_iid_noop => Ok(()),
                None => Err(identity_error()),
            },
            Self::MatchWide(program) => program.accept_identity(step, iid),
            Self::References(program) => program.accept_identity(step, iid),
            Self::Chain(programs) => programs
                .front_mut()
                .ok_or_else(identity_error)?
                .accept_identity(step, iid),
            Self::Compact(_) | Self::Sequential(_) => Err(identity_error()),
        }
    }
}

pub(crate) fn forces_pieces(error: &Diagnostic) -> bool {
    forces_pieces_code(error.code().as_str())
}

pub(crate) fn forces_pieces_code(code: &str) -> bool {
    matches!(
        code,
        "query_plan_binding_limit"
            | "query_plan_write_term_limit"
            | "query_plan_pattern_limit"
            | "migration_assertion_binding_id_out_of_range"
            | "query_plan_input_limit"
            | "query_plan_output_limit"
            | "projected_batch_input_unit_byte_limit"
            | "query_invocation_input_byte_limit"
    )
}

pub(crate) struct WriteStep {
    pub(crate) plan: BuiltPlan,
    pub(crate) complete: bool,
    pub(crate) ordinal: usize,
    expected_iid: Option<String>,
    pub(crate) missing_iid_noop: bool,
    pub(crate) candidate_filter: bool,
    pub(crate) reference_guard: bool,
    pub(crate) reference_resolution: Option<Box<references::ResolutionStep>>,
}
impl WriteStep {
    pub(crate) fn reference_iids(
        &self,
        answer: &PlanAnswer,
    ) -> Result<Vec<(usize, String)>, Diagnostic> {
        let StagedAnswerV3::Rows { rows } = answer.to_staged_v3(self.plan.local())? else {
            return Err(identity_error());
        };
        let QueryOutput::Rows { columns } = self.plan.local().output() else {
            return Err(identity_error());
        };
        if columns.len() % 2 != 0 {
            return Err(identity_error());
        }
        let mut identities = Vec::new();
        for row in rows {
            if row.len() != columns.len() {
                return Err(identity_error());
            }
            for (cells, bindings) in row.chunks_exact(2).zip(columns.chunks_exact(2)) {
                let RemoteValue::Value {
                    value: CanonicalValue::Long(ordinal),
                } = &cells[0]
                else {
                    return Err(identity_error());
                };
                match &cells[1] {
                    RemoteValue::Absent if self.reference_resolution.is_some() => {}
                    RemoteValue::Thing { iid, type_id } => {
                        if !self
                            .plan
                            .local()
                            .binding_types(bindings[1])
                            .is_some_and(|types| types.contains(type_id))
                        {
                            return Err(identity_error());
                        }
                        self.check_identity(iid)?;
                        identities.push((
                            usize::try_from(*ordinal).map_err(|_| identity_error())?,
                            iid.clone(),
                        ));
                    }
                    _ => return Err(identity_error()),
                }
            }
        }
        Ok(identities)
    }

    pub(crate) fn identity(&self, answer: &PlanAnswer) -> Result<Option<String>, Diagnostic> {
        let StagedAnswerV3::Rows { rows } = answer.to_staged_v3(self.plan.local())? else {
            return Err(identity_error());
        };
        if (self.reference_guard || self.missing_iid_noop) && rows.is_empty() {
            return Ok(None);
        }
        let [row] = rows.as_slice() else {
            return Err(identity_error());
        };
        let [
            RemoteValue::Value {
                value: CanonicalValue::Long(ordinal),
            },
            identity,
        ] = row.as_slice()
        else {
            return Err(identity_error());
        };
        if usize::try_from(*ordinal).ok() != Some(self.ordinal) {
            return Err(identity_error());
        }
        match identity {
            RemoteValue::Thing { iid, .. } => {
                self.check_identity(iid)?;
                Ok(Some(iid.clone()))
            }
            RemoteValue::Absent if self.missing_iid_noop => Ok(None),
            _ => Err(identity_error()),
        }
    }

    pub(crate) fn check_identity(&self, iid: &str) -> Result<(), Diagnostic> {
        let target_column = self
            .expected_iid
            .is_none()
            .then(|| {
                self.plan
                    .local()
                    .inputs()
                    .iter()
                    .position(|input| input.public_name().as_str() == "target_iid")
            })
            .flatten();
        let outside_targets = target_column.is_some_and(|column| {
            let mut targets = self
                .plan
                .local()
                .given_rows()
                .into_iter()
                .flatten()
                .filter_map(|row| match row.values()[column].as_ref() {
                    Some(CanonicalValue::String(expected)) => Some(expected.as_str()),
                    _ => None,
                });
            targets.next().is_some_and(|first| {
                !first.eq_ignore_ascii_case(iid)
                    && !targets.any(|target| target.eq_ignore_ascii_case(iid))
            })
        });
        if outside_targets
            || !type_bridge_contract::id::is_canonical_thing_iid(iid)
            || self
                .expected_iid
                .as_deref()
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(iid))
        {
            return Err(identity_error());
        }
        Ok(())
    }
}
fn identity_error() -> Diagnostic {
    invalid(
        "provider_identity_mismatch",
        "A forced write piece returned a different or missing model identity",
    )
}

enum Step {
    Ready(WriteStep),
    Target {
        plan: BuiltPlan,
        complete: bool,
        ordinal: usize,
    },
}
pub(crate) struct SplitProgram {
    steps: VecDeque<Step>,
    authority: PlanAuthority,
    identity: Option<String>,
}
impl SplitProgram {
    pub(crate) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        let Some(step) = self.steps.pop_front() else {
            return Ok(None);
        };
        Ok(Some(match step {
            Step::Ready(step) => step,
            Step::Target {
                plan,
                complete,
                ordinal,
            } => {
                let iid = self.identity.as_deref().ok_or_else(identity_error)?;
                let column = plan
                    .local()
                    .inputs()
                    .iter()
                    .position(|column| column.public_name().as_str() == "target_iid")
                    .ok_or_else(identity_error)?;
                let mut rows = plan
                    .local()
                    .given_rows()
                    .ok_or_else(identity_error)?
                    .to_vec();
                for row in &mut rows {
                    let mut cells = row.values().to_vec();
                    cells[column] = Some(super::iid(iid)?);
                    *row = InputRow::new(cells);
                }
                WriteStep {
                    plan: BuiltPlan {
                        plan: plan
                            .local()
                            .with_given_rows(rows, self.authority.declared())?,
                        verified: plan.verified,
                    },
                    complete,
                    ordinal,
                    expected_iid: Some(iid.to_owned()),
                    missing_iid_noop: false,
                    candidate_filter: false,
                    reference_guard: false,
                    reference_resolution: None,
                }
            }
        }))
    }
    pub(crate) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: Option<String>,
    ) -> Result<(), Diagnostic> {
        if step.reference_guard {
            if iid.is_some() {
                return Ok(());
            }
            return Err(identity_error());
        }
        if let Some(iid) = iid {
            step.check_identity(&iid)?;
            self.identity = Some(iid);
        } else {
            if !step.missing_iid_noop {
                return Err(identity_error());
            }
            while self.steps.front().is_some_and(
                |next| matches!(next, Step::Target { ordinal, .. } if *ordinal == step.ordinal),
            ) {
                self.steps.pop_front();
            }
            self.identity = None;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Player {
    role: RoleId,
    candidates: Vec<TypeId>,
    iid: Option<String>,
    key: Option<(AttributeId, CanonicalValue)>,
}
#[derive(Clone)]
struct Row {
    iid: Option<String>,
    fields: Vec<(OwnsFactId, CanonicalValue)>,
    players: Vec<Player>,
}
impl Row {
    fn of(row: &WriteRow<'_>) -> Self {
        Self {
            iid: row.iid.map(str::to_owned),
            fields: row.fields.to_vec(),
            players: row
                .players
                .iter()
                .map(|(role, reference)| {
                    let (iid, key) = match &reference.identity {
                        ReferenceIdentity::Iid(iid) => (Some((*iid).to_owned()), None),
                        ReferenceIdentity::Key { attribute, value } => {
                            (None, Some(((*attribute).clone(), value.clone())))
                        }
                    };
                    Player {
                        role: role.clone(),
                        candidates: reference.candidates.to_vec(),
                        iid,
                        key,
                    }
                })
                .collect(),
        }
    }
    fn prefix(base: &Self, remaining: &Self, count: usize) -> Self {
        let fields = count.min(remaining.fields.len());
        let players = count.saturating_sub(fields);
        let mut row = base.clone();
        row.fields.extend_from_slice(&remaining.fields[..fields]);
        row.players.extend_from_slice(&remaining.players[..players]);
        row
    }
    fn drain(&mut self, count: usize) {
        let fields = count.min(self.fields.len());
        self.fields.drain(..fields);
        self.players.drain(..count.saturating_sub(fields));
    }
    fn len(&self) -> usize {
        self.fields.len() + self.players.len()
    }
}
fn one(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    row: &Row,
    complete: bool,
    ordinal: usize,
) -> Result<BuiltPlan, Diagnostic> {
    let players = row
        .players
        .iter()
        .map(|player| {
            let identity = if let Some(iid) = &player.iid {
                ReferenceIdentity::Iid(iid)
            } else {
                let (attribute, value) = player.key.as_ref().expect("owned reference identity");
                ReferenceIdentity::Key {
                    attribute,
                    value: value.clone(),
                }
            };
            (
                player.role.clone(),
                Reference {
                    candidates: &player.candidates,
                    identity,
                },
            )
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    if kind == WriteKind::Attach && !shape.single_input {
        for field in &row.fields {
            events.push(WriteRow {
                iid: row.iid.as_deref(),
                fields: std::slice::from_ref(field),
                players: &[],
            });
        }
        for player in &players {
            events.push(WriteRow {
                iid: row.iid.as_deref(),
                fields: &[],
                players: std::slice::from_ref(player),
            });
        }
    }
    if events.is_empty() {
        events.push(WriteRow {
            iid: row.iid.as_deref(),
            fields: &row.fields,
            players: &players,
        });
    }
    let mut plans = build_many(authority, kind, shape, &events, complete, ordinal)?;
    if plans.len() != 1 {
        return Err(invalid(
            "query_plan_input_limit",
            "Write events require canonical input chunks",
        ));
    }
    Ok(plans.remove(0))
}
fn unique_player_identity(authority: &PlanAuthority, player: &Player) -> bool {
    use type_bridge_contract::schema::{
        AnnotationFactId, AnnotationKindId, AnnotationSubjectId, SchemaFact, SchemaFactId,
    };
    if player.iid.is_some() {
        return true;
    }
    let (Some((attribute, _)), Some(first)) = (&player.key, player.candidates.first()) else {
        return false;
    };
    let parents = authority
        .declared()
        .facts()
        .filter_map(|fact| match fact {
            SchemaFact::Sub(fact) => Some((fact.id().subtype(), fact.id().supertype())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let under = |candidate: &TypeId, owner: &TypeId| {
        let mut current = candidate;
        for _ in 0..=parents.len() {
            if current == owner {
                return true;
            }
            let Some(parent) = parents.get(current) else {
                return false;
            };
            current = parent;
        }
        false
    };
    let mut owner = first;
    for _ in 0..=parents.len() {
        let Ok(field) = OwnsFactId::new(owner.clone(), attribute.clone()) else {
            return false;
        };
        if [AnnotationKindId::Key, AnnotationKindId::Unique]
            .into_iter()
            .any(|kind| {
                authority
                    .declared()
                    .fact(&SchemaFactId::Annotation(AnnotationFactId::new(
                        AnnotationSubjectId::Owns(field.clone()),
                        kind,
                    )))
                    .is_some()
            })
            && player
                .candidates
                .iter()
                .all(|candidate| under(candidate, owner))
        {
            return true;
        }
        let Some(parent) = parents.get(owner) else {
            break;
        };
        owner = parent;
    }
    false
}

// Binary search is used only after a fixed canonical limit forces pieces.
fn largest(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    base: &Row,
    remaining: &Row,
    complete: bool,
    ordinal: usize,
) -> Result<usize, Diagnostic> {
    let mut low = 0;
    let mut high = remaining.len();
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        match one(
            authority,
            kind,
            shape.clone(),
            &Row::prefix(base, remaining, middle),
            complete,
            ordinal,
        ) {
            Ok(_) => low = middle,
            Err(error) if forces_pieces(&error) => high = middle - 1,
            Err(error) => return Err(error),
        }
    }
    Ok(low)
}
pub(super) fn split(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<WriteProgram, Diagnostic> {
    split_at(authority, kind, shape, rows, 0)
}

pub(super) fn split_at(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
    ordinal_offset: usize,
) -> Result<WriteProgram, Diagnostic> {
    split_with_guards(authority, kind, shape, rows, ordinal_offset, true, false)
}

pub(super) fn split_resolved(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
) -> Result<WriteProgram, Diagnostic> {
    split_with_guards(authority, kind, shape, rows, 0, false, false)
}

#[allow(clippy::too_many_arguments)]
fn split_with_guards(
    authority: &PlanAuthority,
    kind: WriteKind,
    shape: WriteShape<'_>,
    rows: &[WriteRow<'_>],
    ordinal_offset: usize,
    guard_references: bool,
    known_root: bool,
) -> Result<WriteProgram, Diagnostic> {
    let mut steps = VecDeque::new();
    let mut ordinal = 0;
    while ordinal < rows.len() {
        let output_ordinal = ordinal_offset + ordinal;
        let row = Row::of(&rows[ordinal]);
        match one(authority, kind, shape.clone(), &row, true, output_ordinal) {
            Ok(_) => {
                let mut low = 1;
                let mut high =
                    (rows.len() - ordinal).min(type_bridge_contract::limits::MAX_INPUT_ROWS);
                while low < high {
                    let middle = low + (high - low).div_ceil(2);
                    match build_many(
                        authority,
                        kind,
                        shape.clone(),
                        &rows[ordinal..ordinal + middle],
                        true,
                        output_ordinal,
                    ) {
                        Ok(_) => low = middle,
                        Err(error) if forces_pieces(&error) => high = middle - 1,
                        Err(error) => return Err(error),
                    }
                }
                for plan in build_many(
                    authority,
                    kind,
                    shape.clone(),
                    &rows[ordinal..ordinal + low],
                    true,
                    output_ordinal,
                )? {
                    steps.push_back(Step::Ready(WriteStep {
                        plan,
                        complete: true,
                        ordinal: output_ordinal,
                        expected_iid: None,
                        missing_iid_noop: false,
                        candidate_filter: false,
                        reference_guard: false,
                        reference_resolution: None,
                    }));
                }
                ordinal += low;
                continue;
            }
            Err(error) if forces_pieces(&error) => {}
            Err(error) => return Err(error),
        }
        if matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
            return one(authority, kind, shape.clone(), &row, true, output_ordinal)
                .map(|_| unreachable!("a deletion row exceeded its canonical limits"));
        }
        let (keys, fields) = row
            .fields
            .iter()
            .cloned()
            .partition(|(field, _)| shape.keys.contains(field));
        let seed = Row {
            iid: row.iid.clone(),
            fields: keys,
            players: row.players.first().cloned().into_iter().collect(),
        };
        let mut remaining = Row {
            iid: None,
            fields,
            players: row.players.iter().skip(1).cloned().collect(),
        };
        // All keys and one relation player are present in the allocation piece.
        let cleared_fields = row
            .fields
            .iter()
            .filter(|(field, _)| !shape.keys.contains(field))
            .map(|(field, _)| field.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut seed_shape = shape.clone();
        if matches!(kind, WriteKind::PartialUpdate | WriteKind::PartialPut) {
            seed_shape.cleared_fields = &cleared_fields;
        }
        let plan = one(authority, kind, seed_shape, &seed, false, output_ordinal)?;
        // Prove later references before allocation; witnesses never narrow attachments.
        if guard_references {
            for step in references::guards(
                authority,
                shape.type_id,
                &row.players,
                output_ordinal,
                &plan,
            )? {
                steps.push_back(Step::Ready(step));
            }
        }
        steps.push_back(Step::Ready(WriteStep {
            plan,
            complete: false,
            ordinal: output_ordinal,
            expected_iid: row.iid,
            missing_iid_noop: true,
            candidate_filter: false,
            reference_guard: false,
            reference_resolution: None,
        }));
        let mut attach_shape = shape.clone();
        // Proven single-IID allocations keep the compact event matrix. Only
        // potentially plural sources require fixed-row dependency pieces.
        let single_allocation = shape.type_id.kind() == TypeKind::Entity
            || seed.iid.is_some()
            || (kind != WriteKind::MatchPut
                && seed
                    .fields
                    .iter()
                    .any(|(field, _)| shape.keys.contains(field)))
            || seed
                .players
                .first()
                .is_some_and(|player| unique_player_identity(authority, player));
        attach_shape.single_input = !single_allocation;
        attach_shape.cleared_fields = &[];
        attach_shape.cleared_roles = &[];
        // A maximal canonical IID makes the cut independent of provider output.
        let placeholder = format!(
            "0x{}",
            "0".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS)
        );
        let base = Row {
            iid: if known_root {
                seed.iid.clone()
            } else {
                Some(placeholder)
            },
            fields: vec![],
            players: vec![],
        };
        loop {
            let count = largest(
                authority,
                WriteKind::Attach,
                attach_shape.clone(),
                &base,
                &remaining,
                true,
                output_ordinal,
            )?;
            if count == 0 && remaining.len() != 0 {
                return Err(invalid(
                    "projected_batch_input_unit_byte_limit",
                    "One scalar exceeds the canonical write-piece ceiling",
                ));
            }
            let piece = Row::prefix(&base, &remaining, count);
            remaining.drain(count);
            let complete = remaining.len() == 0;
            let plan = one(
                authority,
                WriteKind::Attach,
                attach_shape.clone(),
                &piece,
                complete,
                output_ordinal,
            )?;
            steps.push_back(if known_root {
                Step::Ready(WriteStep {
                    plan,
                    complete,
                    ordinal: output_ordinal,
                    expected_iid: seed.iid.clone(),
                    missing_iid_noop: true,
                    candidate_filter: false,
                    reference_guard: false,
                    reference_resolution: None,
                })
            } else {
                Step::Target {
                    plan,
                    complete,
                    ordinal: output_ordinal,
                }
            });
            if complete {
                break;
            }
        }
        ordinal += 1;
    }
    Ok(WriteProgram::Split(SplitProgram {
        steps,
        authority: authority.clone(),
        identity: None,
    }))
}
