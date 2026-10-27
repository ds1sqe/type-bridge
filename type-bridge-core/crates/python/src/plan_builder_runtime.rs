//! PyO3 seam for the shared `query-plan/v3` builder.
//!
//! [`PyPlanBuilderCore`] forwards every call to the contract
//! [`PlanBuilder`], so a Python program gives the same canonical bytes as
//! the same program in Rust, TypeScript or C. `type_bridge.plan` passes
//! generated token identities as JSON and handles as dense indexes; this
//! module only decodes them. Like the core builder, calls never fail on
//! content: the first invalid identity, literal or handle is kept and
//! raised by `build`, ahead of the core builder's own first error.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::sync::Arc;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::v3::{
    ArithOp, BuildSelection, BuiltPlans, Builtin, DocumentRole, Expr, PlanBuilder, QueryPlanV3,
    Site, ThingDocument,
};
use type_bridge_contract::query_plan::{InputColumnId, InputRow, QueryOperand};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::query_v2_builder::query_builder_value_type;

use crate::query_v2_builder_runtime::python_scalar;
use crate::query_v2_runtime::{PyQueryV2Authority, value_error};

type ModelBulkCell<'py> = Option<(String, Bound<'py, PyAny>)>;

fn plan_scalar(value_type: ValueTypeTag, value: &Bound<'_, PyAny>) -> PyResult<CanonicalValue> {
    if value_type == ValueTypeTag::Duration && !value.is_instance_of::<pyo3::types::PyString>() {
        return crate::runtime_projection::canonical_plan_duration(value.py(), value);
    }
    python_scalar(value_type, value)
}

fn invalid(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static plan builder diagnostic code"),
        message,
    )
}

fn identity_error(code: &'static str) -> Diagnostic {
    invalid(code, "generated token identity is not canonical")
}

fn handle_error() -> Diagnostic {
    invalid(
        "plan_builder_handle_unknown",
        "a plan handle does not belong to this builder",
    )
}

fn placeholder() -> BindingId {
    BindingId::new(0).expect("zero is a valid binding")
}

fn index(binding: BindingId) -> u32 {
    u32::from(binding.get())
}

/// Parse a closed vocabulary spelling the way the contract serializes it.
fn spelling<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
}

/// One program under construction; `type_bridge.plan.PlanBuilder` is the
/// public surface. Operands are `(kind, index)` pairs, `kind` one of
/// `binding`, `input` or `literal`.
#[pyclass(name = "_PlanBuilderCore", module = "type_bridge_core")]
pub(crate) struct PyPlanBuilderCore {
    core: PlanBuilder,
    // The first Python-side failure; it wins over the core's own.
    error: Option<PyErr>,
    literals: Vec<CanonicalValue>,
    exprs: Vec<Expr>,
}

impl PyPlanBuilderCore {
    fn finish(
        &self,
        py: Python<'_>,
        authority: &PyQueryV2Authority,
        selection: BuildSelection<'_>,
        models: bool,
    ) -> PyResult<PyPlan> {
        let _entry = crate::perf_stages::entry().entered();
        if let Some(error) = &self.error {
            return Err(error.clone_ref(py));
        }
        let authority = authority.authority();
        let semantics = authority
            .context()
            .managed_state()
            .managed_semantic_schema()
            .clone();
        let result = self.core.build_selected_plans(
            authority.declared(),
            semantics,
            selection,
            type_bridge_contract::split::SplitLimits::CANONICAL,
        );
        result
            .and_then(|plan| {
                if models {
                    plan.validate_model_projection()?;
                }
                Ok(plan)
            })
            .map(|plan| PyPlan {
                plans: Some(Arc::new(PyPlanProgram::Raw(Box::new(plan)))),
                decoder: None,
            })
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    fn fail(&mut self, error: PyErr) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn check<T>(&mut self, result: Result<T, Diagnostic>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(diagnostic) => {
                self.fail(value_error(&diagnostic));
                None
            }
        }
    }

    fn binding(&mut self, index: u32) -> BindingId {
        let binding = u16::try_from(index)
            .ok()
            .and_then(|index| BindingId::new(index).ok());
        self.check(binding.ok_or_else(handle_error))
            .unwrap_or_else(placeholder)
    }

    fn type_id(&mut self, type_json: &str) -> Option<TypeId> {
        self.check(
            serde_json::from_str(type_json).map_err(|_| identity_error("invalid_model_identity")),
        )
    }

    fn owns_id(&mut self, owns_json: &str) -> Option<OwnsFactId> {
        let owns = (|| {
            let value: serde_json::Value = serde_json::from_str(owns_json).ok()?;
            let owner: TypeId = serde_json::from_value(value.get("owner")?.clone()).ok()?;
            let attribute = AttributeId::new(value.get("attribute")?.as_str()?).ok()?;
            OwnsFactId::new(owner, attribute).ok()
        })();
        self.check(owns.ok_or_else(|| identity_error("invalid_field_identity")))
    }

    fn role_id(&mut self, role_json: &str) -> Option<RoleId> {
        self.check(
            serde_json::from_str(role_json).map_err(|_| identity_error("invalid_role_identity")),
        )
    }

    fn operand(&mut self, (kind, index): (String, u32)) -> QueryOperand {
        let resolved = match kind.as_str() {
            "binding" => u16::try_from(index)
                .ok()
                .and_then(|index| BindingId::new(index).ok())
                .map(|binding| QueryOperand::Binding { binding }),
            "input" => u16::try_from(index).ok().map(|column| QueryOperand::Input {
                column: InputColumnId::new(column),
            }),
            "literal" => usize::try_from(index)
                .ok()
                .and_then(|index| self.literals.get(index))
                .map(|value| QueryOperand::Literal {
                    value: value.clone(),
                }),
            _ => None,
        };
        self.check(resolved.ok_or_else(handle_error))
            .unwrap_or(QueryOperand::Literal {
                value: CanonicalValue::Boolean(false),
            })
    }

    fn expr(&mut self, index: u32) -> Expr {
        let expr = usize::try_from(index)
            .ok()
            .and_then(|index| self.exprs.get(index))
            .cloned();
        self.check(expr.ok_or_else(handle_error))
            .unwrap_or(Expr::Operand(QueryOperand::Literal {
                value: CanonicalValue::Boolean(false),
            }))
    }

    fn push_expr(&mut self, expr: Expr) -> u32 {
        self.exprs.push(expr);
        u32::try_from(self.exprs.len() - 1).unwrap_or(u32::MAX)
    }
}

#[pymethods]
impl PyPlanBuilderCore {
    #[new]
    #[pyo3(signature = (query_limits=None))]
    fn new(query_limits: Option<PyRef<'_, crate::match_runtime::PyQueryLimits>>) -> Self {
        Self {
            core: PlanBuilder::with_limits(
                query_limits.map_or_else(Default::default, |limits| limits.inner()),
            ),
            error: None,
            literals: Vec::new(),
            exprs: Vec::new(),
        }
    }

    /// Match things of exactly the type identity `type_json`.
    fn exact(&mut self, type_json: &str) -> u32 {
        match self.type_id(type_json) {
            Some(type_id) => index(self.core.exact(type_id)),
            None => 0,
        }
    }

    /// Match things of the type or any subtype.
    fn subtypes(&mut self, type_json: &str) -> u32 {
        match self.type_id(type_json) {
            Some(type_id) => index(self.core.subtypes(type_id)),
            None => 0,
        }
    }

    /// Restrict a matched binding to a canonical IID; core validates its syntax.
    fn by_iid(&mut self, owner: u32, iid: &str) {
        let owner = self.binding(owner);
        self.core.by_iid(owner, iid);
    }

    /// Bind the value of `owner`'s field named by its owns identity.
    fn field(&mut self, owner: u32, owns_json: &str) -> u32 {
        let owner = self.binding(owner);
        match self.owns_id(owns_json) {
            Some(field) => index(self.core.field(owner, field.attribute().clone())),
            None => 0,
        }
    }

    /// Reuse a generated handle's field binding through the shared core.
    fn field_value(&mut self, owner: u32, owns_json: &str) -> u32 {
        let owner = self.binding(owner);
        match self.owns_id(owns_json) {
            Some(field) => index(self.core.field_value(owner, field.attribute().clone())),
            None => 0,
        }
    }

    /// Require `relation` to link `player` in the role `role_json`.
    fn links(&mut self, relation: u32, role_json: &str, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(role_json) {
            self.core.links(relation, role, player);
        }
    }

    /// Require `left comparator right`.
    fn compare(&mut self, left: (String, u32), comparator: &str, right: (String, u32)) {
        let left = self.operand(left);
        let right = self.operand(right);
        if let Some(comparator) = self.check(comparator.parse().map_err(Diagnostic::from)) {
            self.core.compare(left, comparator, right);
        }
    }

    /// Declare a typed input column; returns its column index.
    fn input(&mut self, name: &str, value_type: &str, optional: bool) -> u32 {
        match self.check(query_builder_value_type(value_type)) {
            Some(value_type) => u32::from(self.core.input(name, value_type, optional).get()),
            None => 0,
        }
    }

    /// Copy the recorder so each held write can select its own result terminal.
    fn copy(&self, py: Python<'_>) -> Self {
        Self {
            core: self.core.clone(),
            error: self.error.as_ref().map(|error| error.clone_ref(py)),
            literals: self.literals.clone(),
            exprs: self.exprs.clone(),
        }
    }

    /// Bind a host value using the core's implicit input naming rule.
    fn bind_value(
        &mut self,
        owner: u32,
        field: &str,
        value_type: &str,
        value: &Bound<'_, PyAny>,
    ) -> u32 {
        let owner = self.binding(owner);
        let value = query_builder_value_type(value_type)
            .map_err(|error| value_error(&error))
            .and_then(|value_type| plan_scalar(value_type, value));
        match value {
            Ok(value) => match self.core.bind_value(Site::new(owner, field), value) {
                QueryOperand::Input { column } => u32::from(column.get()),
                _ => unreachable!("bind_value returns an input operand"),
            },
            Err(error) => {
                self.fail(error);
                0
            }
        }
    }

    /// Bind one bulk table; the core owns naming, shape and input-mode checks.
    fn bind_rows(
        &mut self,
        columns: Vec<(u32, String, String)>,
        rows: Vec<Vec<Bound<'_, PyAny>>>,
    ) -> Vec<u32> {
        let mut typed = Vec::with_capacity(columns.len());
        for (owner, field, value_type) in columns {
            let owner = self.binding(owner);
            let Some(value_type) = self.check(query_builder_value_type(&value_type)) else {
                return vec![];
            };
            typed.push((Site::new(owner, field), value_type));
        }
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let mut converted = Vec::with_capacity(row.len());
            for (index, cell) in row.into_iter().enumerate() {
                let Some((_, value_type)) = typed.get(index) else {
                    self.check::<()>(Err(invalid(
                        "plan_builder_row_arity",
                        "bulk input row has too many values",
                    )));
                    return vec![];
                };
                match plan_scalar(*value_type, &cell) {
                    Ok(value) => converted.push(value),
                    Err(error) => {
                        self.fail(error);
                        return vec![];
                    }
                }
            }
            values.push(converted);
        }
        self.core
            .bind_rows(typed, values)
            .into_iter()
            .map(|operand| match operand {
                QueryOperand::Input { column } => u32::from(column.get()),
                _ => unreachable!("bind_rows returns input operands"),
            })
            .collect()
    }

    /// Record generated scalar entity rows; core owns grouping and splitting.
    fn model_rows(&mut self, thing: u32, columns: Vec<String>, rows: Vec<Vec<ModelBulkCell<'_>>>) {
        let thing = self.binding(thing);
        let Some(columns) = columns
            .into_iter()
            .map(|identity| self.owns_id(&identity))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let mut cells = Vec::with_capacity(row.len());
            for cell in row {
                let value = match cell {
                    None => None,
                    Some((value_type, value)) => {
                        let value = query_builder_value_type(&value_type)
                            .map_err(|error| value_error(&error))
                            .and_then(|kind| plan_scalar(kind, &value));
                        match value {
                            Ok(value) => Some(value),
                            Err(error) => {
                                self.fail(error);
                                return;
                            }
                        }
                    }
                };
                cells.push(value);
            }
            values.push(cells);
        }
        self.core.bind_model_columns(thing, columns, values);
    }

    /// Register one canonical literal of `value_type`; returns its index.
    fn literal(&mut self, value_type: &str, value: &Bound<'_, PyAny>) -> u32 {
        let literal = query_builder_value_type(value_type)
            .map_err(|diagnostic| value_error(&diagnostic))
            .and_then(|value_type| plan_scalar(value_type, value));
        let literal = literal.unwrap_or_else(|error| {
            self.fail(error);
            CanonicalValue::Boolean(false)
        });
        self.literals.push(literal);
        u32::try_from(self.literals.len() - 1).unwrap_or(u32::MAX)
    }

    /// An expression of one operand; returns its expression index.
    fn expr_operand(&mut self, operand: (String, u32)) -> u32 {
        let operand = self.operand(operand);
        self.push_expr(Expr::Operand(operand))
    }

    /// `lhs op rhs`, `op` one of `add`, `sub`, `mul`, `div`, `mod`, `pow`.
    fn expr_arith(&mut self, op: &str, lhs: u32, rhs: u32) -> u32 {
        let lhs = self.expr(lhs);
        let rhs = self.expr(rhs);
        let op = spelling::<ArithOp>(op)
            .ok_or_else(|| invalid("plan_builder_expr_invalid", "unknown arithmetic operator"));
        let op = self.check(op).unwrap_or(ArithOp::Add);
        self.push_expr(Expr::Arith {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    /// A builtin call: `abs`, `ceil`, `floor`, `round`, `min`, `max`, `length`.
    fn expr_call(&mut self, func: &str, args: Vec<u32>) -> u32 {
        let args = args.into_iter().map(|arg| self.expr(arg)).collect();
        let func = spelling::<Builtin>(func)
            .ok_or_else(|| invalid("plan_builder_expr_invalid", "unknown builtin function"));
        let func = self.check(func).unwrap_or(Builtin::Abs);
        self.push_expr(Expr::Call { func, args })
    }

    /// Bind `name` to expression `expr`; returns the binding.
    fn let_(&mut self, name: &str, expr: u32) -> u32 {
        let expr = self.expr(expr);
        index(self.core.let_(name, expr))
    }

    /// Assign an expression with a name derived by the shared core.
    fn let_value(&mut self, owner: u32, field: &str, expr: u32) -> u32 {
        let owner = self.binding(owner);
        let expr = self.expr(expr);
        index(self.core.let_value(Site::new(owner, field), expr))
    }

    /// Replace `owner`'s field value with `value`.
    fn update(&mut self, owner: u32, owns_json: &str, value: (String, u32)) {
        let owner = self.binding(owner);
        let value = self.operand(value);
        if let Some(field) = self.owns_id(owns_json) {
            self.core.update(owner, field, value);
        }
    }

    /// Make `player` the role player of `relation`.
    fn update_player(&mut self, relation: u32, role_json: &str, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(role_json) {
            self.core.update_player(relation, role, player);
        }
    }

    /// Add a player through the core attach stage, preserving existing players.
    fn link(&mut self, relation: u32, role_json: &str, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(role_json) {
            self.core.link(relation, role, player);
        }
    }

    /// Create a thing of the type; returns its binding.
    fn insert(&mut self, type_json: &str) -> u32 {
        match self.type_id(type_json) {
            Some(type_id) => index(self.core.insert(type_id)),
            None => 0,
        }
    }

    /// Match a thing with the given fields and players, or create it.
    fn put(&mut self, type_json: &str) -> u32 {
        match self.type_id(type_json) {
            Some(type_id) => index(self.core.put(type_id)),
            None => 0,
        }
    }

    /// Give an inserted or put thing a field value.
    fn thing_field(&mut self, thing: u32, owns_json: &str, value: (String, u32)) {
        let thing = self.binding(thing);
        let value = self.operand(value);
        if let Some(field) = self.owns_id(owns_json) {
            self.core.thing_field(thing, field, value);
        }
    }

    /// Give an inserted or put relation a role player.
    fn thing_player(&mut self, thing: u32, role_json: &str, player: u32) {
        let thing = self.binding(thing);
        let player = self.binding(player);
        if let Some(role) = self.role_id(role_json) {
            self.core.thing_player(thing, role, player);
        }
    }

    /// Delete a thing.
    fn delete(&mut self, thing: u32) {
        let thing = self.binding(thing);
        self.core.delete(thing);
    }

    /// Delete a thing's field, keeping the thing.
    fn delete_field(&mut self, owner: u32, owns_json: &str) {
        let owner = self.binding(owner);
        if let Some(field) = self.owns_id(owns_json) {
            self.core.delete_field(owner, field);
        }
    }

    /// Remove a role player from a relation.
    fn delete_player(&mut self, relation: u32, role_json: &str, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(role_json) {
            self.core.delete_player(relation, role, player);
        }
    }

    /// Add one input row of `(column, literal or None)` cells. Each column
    /// appears at most once; a column left out is absent.
    fn row(&mut self, cells: Vec<(u32, Option<u32>)>) {
        let mut row = vec![None; self.core.input_columns().len()];
        let mut seen = std::collections::BTreeSet::new();
        for (column, literal) in cells {
            let Ok(column) = usize::try_from(column) else {
                self.check::<()>(Err(handle_error()));
                return;
            };
            if column >= row.len() {
                self.check::<()>(Err(handle_error()));
                return;
            }
            if !seen.insert(column) {
                self.check::<()>(Err(invalid(
                    "plan_input_duplicate",
                    "an input row gives one column twice",
                )));
                return;
            }
            let value = match literal {
                None => None,
                Some(literal) => {
                    let value = usize::try_from(literal)
                        .ok()
                        .and_then(|index| self.literals.get(index))
                        .cloned();
                    let Some(value) = self.check(value.ok_or_else(handle_error)) else {
                        return;
                    };
                    Some(value)
                }
            };
            row[column] = value;
        }
        self.core.row(row);
    }

    /// Project these bindings, in order.
    fn returning(&mut self, columns: Vec<u32>) {
        let columns = columns
            .into_iter()
            .map(|column| self.binding(column))
            .collect();
        self.core.returning(columns);
    }

    /// Describe one thing and its listed role players.
    fn fetch(&mut self, thing: u32, roles: Vec<(String, bool)>) {
        let thing = self.binding(thing);
        let roles = roles
            .into_iter()
            .map(|(role, required)| {
                self.role_id(&role)
                    .map(|role| DocumentRole::new(role, required))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(roles) = roles {
            self.core.fetch(thing, roles);
        }
    }

    /// Select a held deletion snapshot after its surrounding block is recorded.
    fn fetch_held(&mut self, thing: u32, roles: Vec<(String, bool)>) {
        let thing = self.binding(thing);
        let roles = roles
            .into_iter()
            .map(|(role, required)| {
                self.role_id(&role)
                    .map(|role| DocumentRole::new(role, required))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(roles) = roles {
            self.core.fetch_held(thing, roles);
        }
    }

    /// Answer the number of rows a read plan matches.
    fn count(&mut self) {
        self.core.count();
    }

    /// Answer whether a read plan matches any row.
    fn exists(&mut self) {
        self.core.exists();
    }

    /// Validate the program under `authority`'s declared schema and managed
    /// semantics; the first invalid call, if any, is the error.
    #[pyo3(signature = (authority, matched=false, models=false))]
    fn build(
        &self,
        py: Python<'_>,
        authority: PyRef<'_, PyQueryV2Authority>,
        matched: bool,
        models: bool,
    ) -> PyResult<PyPlan> {
        self.finish(
            py,
            &authority,
            if matched {
                BuildSelection::Matched
            } else {
                BuildSelection::Recorded
            },
            models,
        )
    }
    /// Build a selected result directly from this immutable recording.
    #[pyo3(signature = (authority, kind, thing, roles, columns, projection=None, declared=None, model_slots=None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "PyO3 keeps the existing selected terminal arguments and its installed model authority explicit"
    )]
    fn build_selected(
        &self,
        py: Python<'_>,
        authority: PyRef<'_, PyQueryV2Authority>,
        kind: &str,
        thing: Option<u32>,
        roles: Vec<(String, bool)>,
        columns: Vec<u32>,
        projection: Option<PyRef<'_, crate::runtime_projection::PyRuntimeProjection>>,
        declared: Option<&str>,
        model_slots: Option<Vec<(u32, String)>>,
    ) -> PyResult<PyPlan> {
        if let Some(error) = &self.error {
            return Err(error.clone_ref(py));
        }
        let binding = |value| {
            u16::try_from(value)
                .ok()
                .and_then(|value| BindingId::new(value).ok())
                .ok_or_else(|| value_error(&handle_error()))
        };
        let columns = columns
            .into_iter()
            .map(binding)
            .collect::<PyResult<Vec<_>>>()?;
        let document = if kind == "models" {
            let thing = binding(thing.ok_or_else(|| value_error(&handle_error()))?)?;
            let roles = roles
                .into_iter()
                .map(|(role, required)| {
                    serde_json::from_str(&role)
                        .map(|role| DocumentRole::new(role, required))
                        .map_err(|_| value_error(&identity_error("invalid_role_identity")))
                })
                .collect::<PyResult<Vec<_>>>()?;
            Some(ThingDocument::new(thing, roles))
        } else {
            None
        };
        let selection = match kind {
            "matched" => BuildSelection::Matched,
            "count" => BuildSelection::Count,
            "exists" => BuildSelection::Exists,
            "rows" => BuildSelection::Rows(&columns),
            "models" => BuildSelection::Model(
                document
                    .as_ref()
                    .expect("model selection creates a document"),
            ),
            _ => {
                return Err(value_error(&invalid(
                    "plan_result_selection_unknown",
                    "the result selection is unknown",
                )));
            }
        };
        let mut finished = self.finish(py, &authority, selection, kind == "models")?;
        if (kind == "models" || model_slots.as_ref().is_some_and(|slots| !slots.is_empty()))
            && let Some(projection) = projection
        {
            let source = finished.plans.take().expect("finished plan source");
            let PyPlanProgram::Raw(plans) = Arc::try_unwrap(source)
                .unwrap_or_else(|_| unreachable!("fresh plan has one owner"))
            else {
                unreachable!("fresh builder returns raw plans")
            };
            let model = if kind == "models" {
                let declared = declared
                    .ok_or_else(|| value_error(&identity_error("invalid_model_identity")))?;
                projection.prepare_authored_plan(
                    *plans,
                    document.as_ref().expect("model document").thing,
                    declared,
                )?
            } else {
                projection.prepare_authored_tuple(*plans, model_slots.unwrap_or_default())?
            };
            finished.plans = Some(Arc::new(PyPlanProgram::Models(Box::new(model))));
        }
        Ok(finished)
    }
}

pub(crate) enum PyPlanProgram {
    Raw(Box<BuiltPlans>),
    Models(Box<crate::runtime_projection::AuthoredPlanProjection>),
}
impl PyPlanProgram {
    pub(crate) fn plans(&self) -> &[QueryPlanV3] {
        match self {
            Self::Raw(plans) => plans.plans(),
            Self::Models(model) => model.stack.plans(),
        }
    }
    fn writes(&self) -> bool {
        match self {
            Self::Raw(plans) => plans.writes(),
            Self::Models(model) => model.stack.writes(),
        }
    }
    pub(crate) fn returns_documents(&self) -> bool {
        match self {
            Self::Raw(plans) => plans.returns_documents(),
            Self::Models(_) => true,
        }
    }
    fn is_bulk(&self) -> bool {
        match self {
            Self::Raw(plans) => plans.bulk_rows().is_some(),
            Self::Models(model) => model.bulk,
        }
    }
}
impl AsRef<[QueryPlanV3]> for PyPlanProgram {
    fn as_ref(&self) -> &[QueryPlanV3] {
        self.plans()
    }
}
impl AsRef<type_bridge_orm::match_request::PlanFetchStack> for PyPlanProgram {
    fn as_ref(&self) -> &type_bridge_orm::match_request::PlanFetchStack {
        let Self::Models(model) = self else {
            unreachable!("only model programs stage fetch groups")
        };
        &model.stack
    }
}

/// One validated `query-plan/v3` program. `type_bridge.plan.Plan`
/// subclasses it; a transaction stages either.
#[pyclass(name = "_Plan", module = "type_bridge_core", subclass)]
pub(crate) struct PyPlan {
    plans: Option<Arc<PyPlanProgram>>,
    pub(crate) decoder: Option<Py<PyAny>>,
}

impl PyPlan {
    pub(crate) fn built(&self) -> PyResult<&Arc<PyPlanProgram>> {
        self.plans.as_ref().ok_or_else(|| {
            value_error(&invalid(
                "plan_not_built",
                "the plan context must finish before the plan is used",
            ))
        })
    }

    pub(crate) fn inner(&self) -> PyResult<&QueryPlanV3> {
        match self.built()?.plans() {
            [plan] => Ok(plan),
            _ => Err(value_error(&invalid(
                "plan_batch_split",
                "a split or empty bulk has no single wire plan",
            ))),
        }
    }
}

#[pymethods]
impl PyPlan {
    /// Copy another plan; lets a Python subclass wrap a built plan.
    #[new]
    #[pyo3(signature = (plan=None))]
    fn new(py: Python<'_>, plan: Option<PyRef<'_, PyPlan>>) -> Self {
        Self {
            plans: plan.as_ref().and_then(|plan| plan.plans.clone()),
            decoder: plan
                .as_ref()
                .and_then(|plan| plan.decoder.as_ref().map(|decoder| decoder.clone_ref(py))),
        }
    }

    /// Finalize a context result once, using an already validated core plan.
    fn _complete(
        &mut self,
        py: Python<'_>,
        plan: PyRef<'_, PyPlan>,
        decoder: Option<Py<PyAny>>,
    ) -> PyResult<()> {
        if self.plans.is_some() {
            return Err(value_error(&invalid(
                "plan_already_built",
                "a built plan is immutable",
            )));
        }
        if decoder
            .as_ref()
            .is_some_and(|decoder| !decoder.bind(py).is_callable())
        {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "plan result decoder must be callable",
            ));
        }
        self.plans = Some(Arc::clone(plan.built()?));
        self.decoder = decoder;
        Ok(())
    }

    /// Rebind a reusable explicit-input plan through the shared contract.
    #[pyo3(signature = (authority, rows, decoder_factory=None))]
    fn bind_rows(
        &self,
        py: Python<'_>,
        authority: PyRef<'_, PyQueryV2Authority>,
        rows: Vec<Vec<(u32, Option<Bound<'_, PyAny>>)>>,
        decoder_factory: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        if self.built()?.is_bulk() {
            return Err(value_error(&invalid(
                "plan_builder_mixed_inputs",
                "bulk plans are rebuilt from their Values rows",
            )));
        }
        let plan = self.inner()?;
        let authority = authority.authority();
        if plan.managed_semantics()
            != authority
                .context()
                .managed_state()
                .managed_semantic_schema()
        {
            return Err(value_error(&invalid(
                "plan_schema_mismatch",
                "binding requires the plan's schema authority",
            )));
        }
        let mut bound = Vec::with_capacity(rows.len());
        for cells in rows {
            let mut values = vec![None; plan.inputs().len()];
            let mut seen = std::collections::BTreeSet::new();
            for (column, value) in cells {
                let column = usize::try_from(column).map_err(|_| value_error(&handle_error()))?;
                let input = plan
                    .inputs()
                    .get(column)
                    .ok_or_else(|| value_error(&handle_error()))?;
                if !seen.insert(column) {
                    return Err(value_error(&invalid(
                        "plan_input_duplicate",
                        "an input row gives one column twice",
                    )));
                }
                values[column] = value
                    .map(|value| plan_scalar(input.value_type(), &value))
                    .transpose()?;
            }
            bound.push(InputRow::new(values));
        }
        let plan = plan
            .with_given_rows(bound, authority.declared())
            .map_err(|error| value_error(&error))?;
        let rebound = BuiltPlans::single(plan);
        let plans = Arc::new(match self.built()?.as_ref() {
            PyPlanProgram::Raw(_) => PyPlanProgram::Raw(Box::new(rebound)),
            PyPlanProgram::Models(model) => {
                PyPlanProgram::Models(Box::new(model.with_plans(rebound)?))
            }
        });
        let decoder = if let Some(factory) = decoder_factory {
            let skeleton = Py::new(
                py,
                Self {
                    plans: Some(Arc::clone(&plans)),
                    decoder: None,
                },
            )?;
            let decoder = factory.call1(py, (skeleton,))?;
            if decoder.is_none(py) {
                None
            } else {
                Some(decoder)
            }
        } else {
            if self.decoder.is_some() {
                return Err(value_error(&invalid(
                    "plan_result_decoder_required",
                    "a typed template needs its result decoder",
                )));
            }
            None
        };
        Ok(Self {
            plans: Some(plans),
            decoder,
        })
    }

    /// The canonical bytes, identical in every SDK.
    fn _scalar_rows(
        &self,
        py: Python<'_>,
        rows: PyRef<'_, crate::staged_transaction_runtime::PyRows>,
    ) -> PyResult<Py<PyAny>> {
        let rows: Vec<serde_json::Value> = pythonize::depythonize(rows.rows.bind(py))?;
        let rows = type_bridge_orm::session::staged::PlanAnswer::Rows(rows)
            .scalar_rows(self.inner()?)
            .map_err(|error| value_error(&error))?;
        crate::runtime_projection::canonical_plan_scalar_rows(py, &rows)
    }

    /// The canonical bytes, identical in every SDK.
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        self.inner()?
            .canonical_bytes()
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    /// Canonical explanation JSON; transported input values are omitted.
    fn explain_json(&self) -> PyResult<String> {
        type_bridge_orm::plan_explain::explain(self.inner()?)
            .and_then(|explanation| explanation.canonical_bytes())
            .map(|bytes| String::from_utf8(bytes).expect("canonical JSON is UTF-8"))
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    /// The plan fingerprint digest in hex.
    fn fingerprint(&self) -> PyResult<String> {
        self.inner()?
            .fingerprint()
            .map(|fingerprint| fingerprint.as_fingerprint().digest().to_hex())
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    /// Whether the plan writes, which makes an inferred kind `write`.
    fn writes(&self) -> PyResult<bool> {
        Ok(self.built()?.writes())
    }
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyPlanBuilderCore>()?;
    module.add_class::<PyPlan>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_orm::query_v2_prepared::QueryAuthority;
    use type_bridge_schema::{
        ManagedDeltaContext, SchemaDocumentSet, build_schema_authority, normalize_documents,
    };

    use crate::purchase_plan::{PROFILE, SCOPE, SOURCE, hand_built_purchase, read_back};

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/plan_builder/purchase.json"
    );

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The Python golden: the Rust SDK's hand-built purchase under the
    /// purchase package authority, which a `QueryV2Authority` over the same
    /// declared bytes, scope and profile must reproduce. The Python unit
    /// test builds the purchase and compares its bytes with this fixture.
    /// `TB_UPDATE_PLAN_BUILDER_FIXTURE=1` rewrites it.
    #[test]
    fn python_purchase_fixture_is_the_rust_hand_built_purchase() {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("purchase.yaml").unwrap(), SOURCE)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new(PROFILE).unwrap();
        let package = build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &ManagedDeltaContext::new(
                ManagedScopeId::new(SCOPE).unwrap(),
                profile,
                CapabilitySet::new(),
            ),
        )
        .unwrap();
        let semantics = package.managed_state().managed_semantic_schema().clone();
        let declared_bytes = encode_declared_schema(&declared).unwrap();
        let authority =
            QueryAuthority::from_declared_bytes(&declared_bytes, SCOPE, PROFILE).unwrap();
        assert_eq!(
            authority
                .context()
                .managed_state()
                .managed_semantic_schema(),
            &semantics
        );
        let read_back_hex = hex(&read_back(&declared, semantics.clone(), true)
            .canonical_bytes()
            .unwrap());
        let read_back_optional_hex = hex(&read_back(&declared, semantics.clone(), false)
            .canonical_bytes()
            .unwrap());
        let purchase = hand_built_purchase(&declared, semantics, "O1", "I1", "A1");
        let fixture = serde_json::json!({
            "scope": SCOPE,
            "profile": PROFILE,
            "declared_hex": hex(&declared_bytes),
            "purchase": { "order": "O1", "item_id": "I1", "account_id": "A1" },
            "purchase_hex": hex(&purchase.canonical_bytes().unwrap()),
            "read_back_hex": read_back_hex,
            "read_back_optional_hex": read_back_optional_hex,
        });
        let expected = format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap());
        if std::env::var_os("TB_UPDATE_PLAN_BUILDER_FIXTURE").is_some() {
            std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
            std::fs::write(FIXTURE, &expected).unwrap();
        }
        assert_eq!(std::fs::read_to_string(FIXTURE).unwrap(), expected);
    }
}
