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
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::v3::{ArithOp, Builtin, Expr, PlanBuilder, QueryPlanV3};
use type_bridge_contract::query_plan::{InputColumnId, QueryOperand};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::query_v2_builder::{query_builder_comparator, query_builder_value_type};

use crate::query_v2_builder_runtime::python_scalar;
use crate::query_v2_runtime::{PyQueryV2Authority, value_error};

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
    fn new() -> Self {
        Self {
            core: PlanBuilder::new(),
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

    /// Bind the value of `owner`'s field named by its owns identity.
    fn field(&mut self, owner: u32, owns_json: &str) -> u32 {
        let owner = self.binding(owner);
        match self.owns_id(owns_json) {
            Some(field) => index(self.core.field(owner, field.attribute().clone())),
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
        if let Some(comparator) = self.check(query_builder_comparator(comparator)) {
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

    /// Register one canonical literal of `value_type`; returns its index.
    fn literal(&mut self, value_type: &str, value: &Bound<'_, PyAny>) -> u32 {
        let literal = query_builder_value_type(value_type)
            .map_err(|diagnostic| value_error(&diagnostic))
            .and_then(|value_type| python_scalar(value_type, value));
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
        let mut row: Vec<Option<CanonicalValue>> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (column, literal) in cells {
            let Ok(column) = usize::try_from(column) else {
                self.check::<()>(Err(handle_error()));
                return;
            };
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
            if row.len() <= column {
                row.resize(column + 1, None);
            }
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
    fn build(&self, py: Python<'_>, authority: PyRef<'_, PyQueryV2Authority>) -> PyResult<PyPlan> {
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
        self.core
            .build(authority.declared(), semantics)
            .map(|plan| PyPlan { plan })
            .map_err(|diagnostic| value_error(&diagnostic))
    }
}

/// One validated `query-plan/v3` program. `type_bridge.plan.Plan`
/// subclasses it; a transaction stages either.
#[pyclass(name = "_Plan", module = "type_bridge_core", subclass, frozen)]
pub(crate) struct PyPlan {
    plan: QueryPlanV3,
}

impl PyPlan {
    pub(crate) const fn inner(&self) -> &QueryPlanV3 {
        &self.plan
    }
}

#[pymethods]
impl PyPlan {
    /// Copy another plan; lets a Python subclass wrap a built plan.
    #[new]
    fn new(plan: PyRef<'_, PyPlan>) -> Self {
        Self {
            plan: plan.plan.clone(),
        }
    }

    /// The canonical bytes, identical in every SDK.
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        self.plan
            .canonical_bytes()
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    /// The plan fingerprint digest in hex.
    fn fingerprint(&self) -> PyResult<String> {
        self.plan
            .fingerprint()
            .map(|fingerprint| fingerprint.as_fingerprint().digest().to_hex())
            .map_err(|diagnostic| value_error(&diagnostic))
    }

    /// Whether the plan writes, which makes an inferred kind `write`.
    fn writes(&self) -> bool {
        self.plan.writes()
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

    use crate::purchase_plan::{PROFILE, SCOPE, SOURCE, hand_built_purchase};

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
        let purchase = hand_built_purchase(&declared, semantics, "O1", "I1", "A1");
        let fixture = serde_json::json!({
            "scope": SCOPE,
            "profile": PROFILE,
            "declared_hex": hex(&declared_bytes),
            "purchase": { "order": "O1", "item_id": "I1", "account_id": "A1" },
            "purchase_hex": hex(&purchase.canonical_bytes().unwrap()),
        });
        let expected = format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap());
        if std::env::var_os("TB_UPDATE_PLAN_BUILDER_FIXTURE").is_some() {
            std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
            std::fs::write(FIXTURE, &expected).unwrap();
        }
        assert_eq!(std::fs::read_to_string(FIXTURE).unwrap(), expected);
    }
}
