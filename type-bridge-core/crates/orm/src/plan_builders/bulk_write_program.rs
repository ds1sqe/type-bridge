//! Fixed-limit pieces of one manager write, all executed by its existing lease.
use super::*;
use crate::query_plan_exec::PlanAnswer;
use std::collections::VecDeque;
use type_bridge_contract::query_remote::RemoteValue;
use type_bridge_contract::query_remote_v3::StagedAnswerV3;

pub(crate) enum WriteProgram {
    Compact(Vec<BuiltPlan>),
    Sequential(Vec<(BuiltPlan, Vec<usize>)>),
    Split(SplitProgram),
}
impl WriteProgram {
    pub(crate) fn compact(plans: Vec<BuiltPlan>) -> Self {
        Self::Compact(plans)
    }
    pub(crate) fn statement_count(&self) -> usize {
        match self {
            Self::Compact(plans) => plans.len(),
            Self::Sequential(plans) => plans.len(),
            Self::Split(program) => program.steps.len(),
        }
    }
    pub(crate) fn next(&mut self) -> Result<Option<WriteStep>, Diagnostic> {
        match self {
            Self::Compact(plans) => Ok((!plans.is_empty()).then(|| WriteStep {
                plan: plans.remove(0),
                complete: true,
                ordinal: 0,
                expected_iid: None,
            })),
            Self::Sequential(plans) => Ok((!plans.is_empty()).then(|| WriteStep {
                plan: plans.remove(0).0,
                complete: true,
                ordinal: 0,
                expected_iid: None,
            })),
            Self::Split(program) => program.next(),
        }
    }
    pub(crate) fn into_static(self) -> Result<Vec<BuiltPlan>, Diagnostic> {
        match self {
            Self::Compact(plans) => Ok(plans),
            Self::Sequential(plans) => Ok(plans.into_iter().map(|(plan, _)| plan).collect()),
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

    pub(crate) fn identity_layout(&self) -> Option<Vec<Vec<(usize, String)>>> {
        let Self::Sequential(plans) = self else {
            return None;
        };
        Some(
            plans
                .iter()
                .map(|(plan, ordinals)| {
                    let QueryOutput::Rows { columns } = plan.local().output() else {
                        unreachable!("sequential puts return row identities")
                    };
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
                        .collect()
                })
                .collect(),
        )
    }

    pub(crate) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: String,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::Split(program) => program.accept_identity(step, iid),
            Self::Compact(_) | Self::Sequential(_) => Err(identity_error()),
        }
    }
}

pub(crate) fn forces_pieces(error: &Diagnostic) -> bool {
    matches!(
        error.code().as_str(),
        "query_plan_binding_limit"
            | "migration_assertion_binding_id_out_of_range"
            | "query_plan_input_limit"
            | "projected_batch_input_unit_byte_limit"
            | "query_invocation_input_byte_limit"
    )
}

pub(crate) struct WriteStep {
    pub(crate) plan: BuiltPlan,
    pub(crate) complete: bool,
    pub(crate) ordinal: usize,
    expected_iid: Option<String>,
}
impl WriteStep {
    pub(crate) fn identity(&self, answer: &PlanAnswer) -> Result<String, Diagnostic> {
        let StagedAnswerV3::Rows { rows } = answer.to_staged_v3(self.plan.local())? else {
            return Err(identity_error());
        };
        let [row] = rows.as_slice() else {
            return Err(identity_error());
        };
        let [
            RemoteValue::Value {
                value: CanonicalValue::Long(ordinal),
            },
            RemoteValue::Thing { iid, .. },
        ] = row.as_slice()
        else {
            return Err(identity_error());
        };
        if usize::try_from(*ordinal).ok() != Some(self.ordinal) {
            return Err(identity_error());
        }
        self.check_identity(iid)?;
        Ok(iid.clone())
    }
    pub(crate) fn check_identity(&self, iid: &str) -> Result<(), Diagnostic> {
        if !type_bridge_contract::id::is_canonical_thing_iid(iid)
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
                }
            }
        }))
    }
    pub(crate) fn accept_identity(
        &mut self,
        step: &WriteStep,
        iid: String,
    ) -> Result<(), Diagnostic> {
        step.check_identity(&iid)?;
        self.identity = Some(iid);
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
    if kind == WriteKind::Attach {
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
    let mut steps = VecDeque::new();
    let mut ordinal = 0;
    while ordinal < rows.len() {
        let row = Row::of(&rows[ordinal]);
        match one(authority, kind, shape.clone(), &row, true, ordinal) {
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
                        ordinal,
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
                    ordinal,
                )? {
                    steps.push_back(Step::Ready(WriteStep {
                        plan,
                        complete: true,
                        ordinal,
                        expected_iid: None,
                    }));
                }
                ordinal += low;
                continue;
            }
            Err(error) if forces_pieces(&error) => {}
            Err(error) => return Err(error),
        }
        if matches!(kind, WriteKind::Delete | WriteKind::DeleteReturning) {
            return one(authority, kind, shape.clone(), &row, true, ordinal)
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
        let plan = one(authority, kind, seed_shape, &seed, false, ordinal)?;
        steps.push_back(Step::Ready(WriteStep {
            plan,
            complete: false,
            ordinal,
            expected_iid: row.iid,
        }));
        let mut attach_shape = shape.clone();
        attach_shape.cleared_fields = &[];
        attach_shape.cleared_roles = &[];
        // A maximal canonical IID makes the cut independent of provider output.
        let placeholder = format!(
            "0x{}",
            "0".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS)
        );
        let base = Row {
            iid: Some(placeholder),
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
                ordinal,
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
                ordinal,
            )?;
            steps.push_back(Step::Target {
                plan,
                complete,
                ordinal,
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
