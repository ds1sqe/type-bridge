//! N-API seam for the shared `query-plan/v3` builder.
//!
//! [`NodePlanBuilder`] forwards every call to the contract
//! [`PlanBuilder`], so a TypeScript program gives the same canonical bytes
//! as the same program in Rust, Python or C. JavaScript passes generated
//! token identities as canonical JSON and handles as dense indexes; this
//! module only decodes them. Like the core builder, calls never fail on
//! content: the first invalid identity, literal or handle is kept and
//! returned by `build`, ahead of the core builder's own first error.

use napi::bindgen_prelude::{Buffer, Env, Unknown};
use napi_derive::napi;
use std::sync::Arc;
use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::v3::{
    ArithOp, BuildSelection, BuiltPlans, Builtin, DocumentRole, Expr, PlanBuilder, QueryPlanV3,
    Site, ThingDocument,
};
use type_bridge_contract::query_plan::{InputColumnId, QueryOperand};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::query_v2_builder::query_builder_value_type;

use crate::query_v2_runtime::{NodeQueryV2Authority, napi_error};

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

/// One operand as JavaScript passes it: a binding, an input column, or a
/// literal created by [`NodePlanBuilder::literal`].
#[napi(object)]
pub struct NodePlanOperand {
    /// `binding`, `input` or `literal`.
    pub kind: String,
    /// The binding, input column or literal index.
    pub index: u32,
}

/// One ownership and scalar domain shared by every generated bulk row.
#[napi(object)]
pub struct NodePlanModelColumn {
    pub field: String,
    pub value_type: String,
}

/// One implicit bulk input site and scalar domain.
#[napi(object)]
pub struct NodePlanColumn {
    pub owner: u32,
    pub field: String,
    pub value_type: String,
}

/// One program under construction; see the module documentation.
#[napi]
pub struct NodePlanBuilder {
    core: PlanBuilder,
    // The first JavaScript-side failure; it wins over the core's own.
    error: Option<(napi::Status, String)>,
    literals: Vec<CanonicalValue>,
    exprs: Vec<Expr>,
}

impl NodePlanBuilder {
    fn finish(
        &self,
        authority: &NodeQueryV2Authority,
        selection: BuildSelection<'_>,
        models: bool,
    ) -> napi::Result<NodePlan> {
        let _entry = crate::perf_stages::entry().entered();
        if let Some((status, reason)) = &self.error {
            return Err(napi::Error::new(*status, reason.clone()));
        }
        let authority = authority.authority();
        let semantics = authority
            .context()
            .managed_state()
            .managed_semantic_schema()
            .clone();
        self.core
            .build_selected_plans(
                authority.declared(),
                semantics,
                selection,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .and_then(|plans| {
                if models {
                    plans.validate_model_projection()?;
                }
                Ok(plans)
            })
            .map(|plans| NodePlan {
                plans: Arc::new(NodePlanProgram::Raw(Box::new(plans))),
            })
            .map_err(|diagnostic| napi_error(&diagnostic))
    }

    fn fail(&mut self, error: napi::Error) {
        self.error.get_or_insert((error.status, error.reason));
    }

    fn check<T>(&mut self, result: Result<T, Diagnostic>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(diagnostic) => {
                self.fail(napi_error(&diagnostic));
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
            from_canonical_json(type_json.as_bytes())
                .map_err(|_| identity_error("invalid_model_identity")),
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
            from_canonical_json(role_json.as_bytes())
                .map_err(|_| identity_error("invalid_role_identity")),
        )
    }

    fn operand(&mut self, operand: &NodePlanOperand) -> QueryOperand {
        let resolved = match operand.kind.as_str() {
            "binding" => u16::try_from(operand.index)
                .ok()
                .and_then(|index| BindingId::new(index).ok())
                .map(|binding| QueryOperand::Binding { binding }),
            "input" => u16::try_from(operand.index)
                .ok()
                .map(|column| QueryOperand::Input {
                    column: InputColumnId::new(column),
                }),
            "literal" => usize::try_from(operand.index)
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

#[napi]
impl NodePlanBuilder {
    /// Copy a recording before selecting an independent result.
    #[napi]
    pub fn copy(&self) -> Self {
        Self {
            core: self.core.clone(),
            error: self.error.clone(),
            literals: self.literals.clone(),
            exprs: self.exprs.clone(),
        }
    }

    /// An empty program.
    #[napi(constructor)]
    pub fn new(query_limits: Option<&crate::match_runtime::NodeQueryLimits>) -> Self {
        Self {
            core: PlanBuilder::with_limits(
                query_limits.map_or_else(Default::default, |limits| limits.inner()),
            ),
            error: None,
            literals: Vec::new(),
            exprs: Vec::new(),
        }
    }

    /// Match things of exactly the canonical type identity `type_json`.
    #[napi]
    pub fn exact(&mut self, type_json: String) -> u32 {
        match self.type_id(&type_json) {
            Some(type_id) => index(self.core.exact(type_id)),
            None => 0,
        }
    }

    /// Match things of the type or any subtype.
    #[napi]
    pub fn subtypes(&mut self, type_json: String) -> u32 {
        match self.type_id(&type_json) {
            Some(type_id) => index(self.core.subtypes(type_id)),
            None => 0,
        }
    }

    /// Bind the value of `owner`'s field named by its declaring owns identity.
    #[napi]
    pub fn field(&mut self, owner: u32, owns_json: String) -> u32 {
        let owner = self.binding(owner);
        match self.owns_id(&owns_json) {
            Some(field) => index(self.core.field(owner, field.attribute().clone())),
            None => 0,
        }
    }

    /// Reuse one generated field binding per owner and attribute.
    #[napi(js_name = "fieldValue")]
    pub fn field_value(&mut self, owner: u32, owns_json: String) -> u32 {
        let owner = self.binding(owner);
        match self.owns_id(&owns_json) {
            Some(field) => index(self.core.field_value(owner, field.attribute().clone())),
            None => 0,
        }
    }

    /// Constrain a thing by its provider identifier.
    #[napi(js_name = "byIid")]
    pub fn by_iid(&mut self, owner: u32, iid: String) {
        let owner = self.binding(owner);
        self.core.by_iid(owner, &iid);
    }

    /// Add a role player while preserving existing players.
    #[napi]
    pub fn link(&mut self, relation: u32, role_json: String, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(&role_json) {
            self.core.link(relation, role, player);
        }
    }

    /// Bind a caller value using the shared derived input name.
    #[napi(js_name = "bindValue")]
    pub fn bind_value(
        &mut self,
        env: Env,
        owner: u32,
        field: String,
        value_type: String,
        value: Unknown,
    ) -> u32 {
        let owner = self.binding(owner);
        let parsed = query_builder_value_type(&value_type)
            .map_err(|error| napi_error(&error))
            .and_then(|kind| crate::query_v2_builder_runtime::node_scalar(&env, kind, value));
        match parsed {
            Ok(value) => match self.core.bind_value(Site::new(owner, &field), value) {
                QueryOperand::Input { column } => u32::from(column.get()),
                _ => unreachable!("bind_value returns an input operand"),
            },
            Err(error) => {
                self.fail(error);
                0
            }
        }
    }

    /// Name an assignment expression through the shared site namespace.
    #[napi(js_name = "letValue")]
    pub fn let_value(&mut self, owner: u32, field: String, expr: u32) -> u32 {
        let owner = self.binding(owner);
        let expr = self.expr(expr);
        index(self.core.let_value(Site::new(owner, &field), expr))
    }

    /// Supply entity Values rows; grouping, limits and splitting stay in core.
    #[napi(js_name = "modelRows")]
    pub fn model_rows(
        &mut self,
        env: Env,
        thing: u32,
        columns: Vec<NodePlanModelColumn>,
        rows: Vec<Vec<Unknown>>,
    ) {
        let thing = self.binding(thing);
        let mut fields = Vec::with_capacity(columns.len());
        let mut kinds = Vec::with_capacity(columns.len());
        for column in columns {
            let Some(field) = self.owns_id(&column.field) else {
                return;
            };
            let Some(kind) = self.check(query_builder_value_type(&column.value_type)) else {
                return;
            };
            fields.push(field);
            kinds.push(kind);
        }
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            if row.len() != kinds.len() {
                self.check::<()>(Err(invalid(
                    "plan_builder_row_arity",
                    "every bulk input row must have one cell per column",
                )));
                return;
            }
            let mut cells = Vec::with_capacity(row.len());
            for (value, kind) in row.into_iter().zip(&kinds) {
                let value = match value.get_type() {
                    Ok(napi::ValueType::Undefined) => None,
                    Ok(_) => match crate::query_v2_builder_runtime::node_scalar(&env, *kind, value)
                    {
                        Ok(value) => Some(value),
                        Err(error) => {
                            self.fail(error);
                            return;
                        }
                    },
                    Err(error) => {
                        self.fail(error);
                        return;
                    }
                };
                cells.push(value);
            }
            values.push(cells);
        }
        self.core.bind_model_columns(thing, fields, values);
    }

    /// Bind one implicit input table through the shared builder.
    #[napi(js_name = "bindRows")]
    pub fn bind_rows(&mut self, columns: Vec<NodePlanColumn>, rows: Vec<Vec<u32>>) -> Vec<u32> {
        let mut typed = Vec::with_capacity(columns.len());
        for column in columns {
            let owner = self.binding(column.owner);
            let Some(kind) = self.check(query_builder_value_type(&column.value_type)) else {
                return vec![];
            };
            typed.push((Site::new(owner, column.field), kind));
        }
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let mut cells = Vec::with_capacity(row.len());
            for literal in row {
                let value = usize::try_from(literal)
                    .ok()
                    .and_then(|index| self.literals.get(index))
                    .cloned();
                let Some(value) = self.check(value.ok_or_else(handle_error)) else {
                    return vec![];
                };
                cells.push(value);
            }
            values.push(cells);
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

    /// Require `relation` to link `player` in the role `role_json`.
    #[napi]
    pub fn links(&mut self, relation: u32, role_json: String, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(&role_json) {
            self.core.links(relation, role, player);
        }
    }

    /// Require `left comparator right`.
    #[napi]
    pub fn compare(&mut self, left: NodePlanOperand, comparator: String, right: NodePlanOperand) {
        let left = self.operand(&left);
        let right = self.operand(&right);
        if let Some(comparator) = self.check(comparator.parse().map_err(Diagnostic::from)) {
            self.core.compare(left, comparator, right);
        }
    }

    /// Declare a typed input column; returns its column index.
    #[napi]
    pub fn input(&mut self, name: String, value_type: String, optional: bool) -> u32 {
        match self.check(query_builder_value_type(&value_type)) {
            Some(value_type) => u32::from(self.core.input(&name, value_type, optional).get()),
            None => 0,
        }
    }

    /// Register one canonical literal of `value_type`; returns its index.
    #[napi]
    pub fn literal(&mut self, env: Env, value_type: String, value: Unknown) -> u32 {
        let literal = query_builder_value_type(&value_type)
            .map_err(|diagnostic| napi_error(&diagnostic))
            .and_then(|value_type| {
                crate::query_v2_builder_runtime::node_scalar(&env, value_type, value)
            });
        let literal = match literal {
            Ok(literal) => literal,
            Err(error) => {
                self.fail(error);
                CanonicalValue::Boolean(false)
            }
        };
        self.literals.push(literal);
        u32::try_from(self.literals.len() - 1).unwrap_or(u32::MAX)
    }

    /// An expression of one operand; returns its expression index.
    #[napi(js_name = "exprOperand")]
    pub fn expr_operand(&mut self, operand: NodePlanOperand) -> u32 {
        let operand = self.operand(&operand);
        self.push_expr(Expr::Operand(operand))
    }

    /// `lhs op rhs`, `op` one of `add`, `sub`, `mul`, `div`, `mod`, `pow`.
    #[napi(js_name = "exprArith")]
    pub fn expr_arith(&mut self, op: String, lhs: u32, rhs: u32) -> u32 {
        let lhs = self.expr(lhs);
        let rhs = self.expr(rhs);
        let op = spelling::<ArithOp>(&op)
            .ok_or_else(|| invalid("plan_builder_expr_invalid", "unknown arithmetic operator"));
        let op = self.check(op).unwrap_or(ArithOp::Add);
        self.push_expr(Expr::Arith {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    /// A builtin call: `abs`, `ceil`, `floor`, `round`, `min`, `max`, `length`.
    #[napi(js_name = "exprCall")]
    pub fn expr_call(&mut self, func: String, args: Vec<u32>) -> u32 {
        let args = args.into_iter().map(|arg| self.expr(arg)).collect();
        let func = spelling::<Builtin>(&func)
            .ok_or_else(|| invalid("plan_builder_expr_invalid", "unknown builtin function"));
        let func = self.check(func).unwrap_or(Builtin::Abs);
        self.push_expr(Expr::Call { func, args })
    }

    /// Bind `name` to expression `expr`.
    #[napi(js_name = "let")]
    pub fn let_(&mut self, name: String, expr: u32) -> u32 {
        let expr = self.expr(expr);
        index(self.core.let_(&name, expr))
    }

    /// Replace `owner`'s field value with `value`.
    #[napi]
    pub fn update(&mut self, owner: u32, owns_json: String, value: NodePlanOperand) {
        let owner = self.binding(owner);
        let value = self.operand(&value);
        if let Some(field) = self.owns_id(&owns_json) {
            self.core.update(owner, field, value);
        }
    }

    /// Make `player` the role player of `relation`.
    #[napi(js_name = "updatePlayer")]
    pub fn update_player(&mut self, relation: u32, role_json: String, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(&role_json) {
            self.core.update_player(relation, role, player);
        }
    }

    /// Create a thing of the type; returns its binding.
    #[napi]
    pub fn insert(&mut self, type_json: String) -> u32 {
        match self.type_id(&type_json) {
            Some(type_id) => index(self.core.insert(type_id)),
            None => 0,
        }
    }

    /// Match a thing with the given fields and players, or create it.
    #[napi]
    pub fn put(&mut self, type_json: String) -> u32 {
        match self.type_id(&type_json) {
            Some(type_id) => index(self.core.put(type_id)),
            None => 0,
        }
    }

    /// Give an inserted or put thing a field value.
    #[napi(js_name = "thingField")]
    pub fn thing_field(&mut self, thing: u32, owns_json: String, value: NodePlanOperand) {
        let thing = self.binding(thing);
        let value = self.operand(&value);
        if let Some(field) = self.owns_id(&owns_json) {
            self.core.thing_field(thing, field, value);
        }
    }

    /// Give an inserted or put relation a role player.
    #[napi(js_name = "thingPlayer")]
    pub fn thing_player(&mut self, thing: u32, role_json: String, player: u32) {
        let thing = self.binding(thing);
        let player = self.binding(player);
        if let Some(role) = self.role_id(&role_json) {
            self.core.thing_player(thing, role, player);
        }
    }

    /// Delete a thing.
    #[napi]
    pub fn delete(&mut self, thing: u32) {
        let thing = self.binding(thing);
        self.core.delete(thing);
    }

    /// Delete a thing's field, keeping the thing.
    #[napi(js_name = "deleteField")]
    pub fn delete_field(&mut self, owner: u32, owns_json: String) {
        let owner = self.binding(owner);
        if let Some(field) = self.owns_id(&owns_json) {
            self.core.delete_field(owner, field);
        }
    }

    /// Remove a role player from a relation.
    #[napi(js_name = "deletePlayer")]
    pub fn delete_player(&mut self, relation: u32, role_json: String, player: u32) {
        let relation = self.binding(relation);
        let player = self.binding(player);
        if let Some(role) = self.role_id(&role_json) {
            self.core.delete_player(relation, role, player);
        }
    }

    /// Add one input row: `columns[i]` takes literal `literals[i]`, or is
    /// absent when that literal is `null`. Each column appears at most
    /// once; a column left out is absent.
    #[napi]
    pub fn row(&mut self, columns: Vec<u32>, literals: Vec<Option<u32>>) {
        if columns.len() != literals.len() {
            self.check::<()>(Err(invalid(
                "query_invocation_row_arity",
                "each input cell needs its column",
            )));
            return;
        }
        let mut row = vec![None; self.core.input_columns().len()];
        let mut seen = std::collections::BTreeSet::new();
        for (column, literal) in columns.into_iter().zip(literals) {
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
    #[napi]
    pub fn returning(&mut self, columns: Vec<u32>) {
        let columns = columns
            .into_iter()
            .map(|column| self.binding(column))
            .collect();
        self.core.returning(columns);
    }

    /// Describe one thing and its listed role players.
    #[napi]
    pub fn fetch(&mut self, thing: u32, roles: Vec<String>, required: Vec<bool>) {
        if roles.len() != required.len() {
            self.check::<()>(Err(invalid(
                "plan_fetch_roles_invalid",
                "each role needs its required flag",
            )));
            return;
        }
        let thing = self.binding(thing);
        let roles = roles
            .into_iter()
            .zip(required)
            .map(|(role, required)| {
                self.role_id(&role)
                    .map(|role| DocumentRole::new(role, required))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(roles) = roles {
            self.core.fetch(thing, roles);
        }
    }

    /// Select a held model result after the complete recording is known.
    #[napi(js_name = "fetchHeld")]
    pub fn fetch_held(&mut self, thing: u32, roles: Vec<String>, required: Vec<bool>) {
        if roles.len() != required.len() {
            self.check::<()>(Err(invalid(
                "plan_fetch_roles_invalid",
                "each role needs its required flag",
            )));
            return;
        }
        let thing = self.binding(thing);
        let roles = roles
            .into_iter()
            .zip(required)
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
    #[napi]
    pub fn count(&mut self) {
        self.core.count();
    }

    /// Answer whether a read plan matches any row.
    #[napi]
    pub fn exists(&mut self) {
        self.core.exists();
    }

    /// Validate the program under `authority`'s declared schema and managed
    /// semantics; the first invalid call, if any, is the error.
    #[napi]
    pub fn build(
        &self,
        authority: &NodeQueryV2Authority,
        matched: Option<bool>,
        models: Option<bool>,
    ) -> napi::Result<NodePlan> {
        self.finish(
            authority,
            if matched.unwrap_or(false) {
                BuildSelection::Matched
            } else {
                BuildSelection::Recorded
            },
            models.unwrap_or(false),
        )
    }
    /// Build one selected result without copying the recorder.
    #[napi]
    #[allow(clippy::too_many_arguments)] // napi forwards attributes to wrappers with different arities.
    pub fn build_selected(
        &self,
        authority: &NodeQueryV2Authority,
        kind: String,
        thing: Option<u32>,
        roles: Vec<String>,
        required: Vec<bool>,
        columns: Vec<u32>,
        projection: Option<&crate::runtime_projection::NodeRuntimeProjection>,
        declared: Option<String>,
        model_bindings: Option<Vec<u32>>,
        model_types: Option<Vec<String>>,
    ) -> napi::Result<NodePlan> {
        if let Some((status, reason)) = &self.error {
            return Err(napi::Error::new(*status, reason.clone()));
        }
        let binding = |value| {
            u16::try_from(value)
                .ok()
                .and_then(|value| BindingId::new(value).ok())
                .ok_or_else(|| napi_error(&handle_error()))
        };
        let columns = columns
            .into_iter()
            .map(binding)
            .collect::<napi::Result<Vec<_>>>()?;
        let document = if kind == "models" {
            let thing = binding(thing.ok_or_else(|| napi_error(&handle_error()))?)?;
            if roles.len() != required.len() {
                return Err(napi_error(&handle_error()));
            }
            let roles = roles
                .into_iter()
                .zip(required)
                .map(|(role, required)| {
                    from_canonical_json(role.as_bytes())
                        .map(|role| DocumentRole::new(role, required))
                        .map_err(|_| napi_error(&identity_error("invalid_role_identity")))
                })
                .collect::<napi::Result<Vec<_>>>()?;
            Some(ThingDocument::new(thing, roles))
        } else {
            None
        };
        let selection = match kind.as_str() {
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
                return Err(napi_error(&invalid(
                    "plan_result_selection_unknown",
                    "the result selection is unknown",
                )));
            }
        };
        let mut finished = self.finish(authority, selection, kind == "models")?;
        if (kind == "models"
            || model_bindings
                .as_ref()
                .is_some_and(|slots| !slots.is_empty()))
            && let Some(projection) = projection
        {
            let NodePlanProgram::Raw(plans) = Arc::try_unwrap(finished.plans)
                .unwrap_or_else(|_| unreachable!("fresh plan has one owner"))
            else {
                unreachable!("fresh builder returns raw plans")
            };
            let model = if kind == "models" {
                let declared = declared
                    .ok_or_else(|| napi_error(&identity_error("invalid_model_identity")))?;
                projection.prepare_authored_plan(
                    *plans,
                    document.as_ref().expect("model document").thing,
                    &declared,
                )?
            } else {
                projection.prepare_authored_tuple(
                    *plans,
                    model_bindings.unwrap_or_default(),
                    model_types.unwrap_or_default(),
                )?
            };
            finished.plans = Arc::new(NodePlanProgram::Models(Box::new(model)));
        }
        Ok(finished)
    }
}

impl Default for NodePlanBuilder {
    fn default() -> Self {
        Self::new(None)
    }
}

pub(crate) enum NodePlanProgram {
    Raw(Box<BuiltPlans>),
    Models(Box<crate::runtime_projection::AuthoredPlanProjection>),
}
impl NodePlanProgram {
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
impl AsRef<[QueryPlanV3]> for NodePlanProgram {
    fn as_ref(&self) -> &[QueryPlanV3] {
        self.plans()
    }
}
impl AsRef<type_bridge_orm::match_request::PlanFetchStack> for NodePlanProgram {
    fn as_ref(&self) -> &type_bridge_orm::match_request::PlanFetchStack {
        let Self::Models(model) = self else {
            unreachable!("only model programs stage fetch groups")
        };
        &model.stack
    }
}

/// One validated `query-plan/v3` program.
#[napi]
pub struct NodePlan {
    pub(crate) plans: Arc<NodePlanProgram>,
}

impl NodePlan {
    fn inner(&self) -> napi::Result<&QueryPlanV3> {
        match self.plans.plans() {
            [plan] => Ok(plan),
            _ => Err(napi_error(&invalid(
                "plan_batch_split",
                "a split or empty bulk has no single wire plan",
            ))),
        }
    }
}

#[napi]
impl NodePlan {
    /// Bind a reusable explicit template to a fresh validated invocation.
    #[napi(js_name = "bindRows")]
    pub fn bind_rows(
        &self,
        env: Env,
        authority: &NodeQueryV2Authority,
        columns: Vec<Vec<u32>>,
        rows: Vec<Vec<Unknown<'_>>>,
    ) -> napi::Result<Self> {
        if self.plans.is_bulk() {
            return Err(napi_error(&invalid(
                "plan_builder_mixed_inputs",
                "bulk plans are rebuilt from Values rows",
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
            return Err(napi_error(&invalid(
                "plan_schema_mismatch",
                "binding requires the plan's schema authority",
            )));
        }
        if columns.len() != rows.len() {
            return Err(napi_error(&handle_error()));
        }
        let mut bound = Vec::with_capacity(rows.len());
        for (columns, row) in columns.into_iter().zip(rows) {
            if columns.len() != row.len() {
                return Err(napi_error(&handle_error()));
            }
            let mut values = vec![None; plan.inputs().len()];
            let mut seen = std::collections::BTreeSet::new();
            for (column, value) in columns.into_iter().zip(row) {
                let column = usize::try_from(column).map_err(|_| napi_error(&handle_error()))?;
                let input = plan
                    .inputs()
                    .get(column)
                    .ok_or_else(|| napi_error(&handle_error()))?;
                if !seen.insert(column) {
                    return Err(napi_error(&invalid(
                        "plan_input_duplicate",
                        "an input row gives one column twice",
                    )));
                }
                if value.get_type()? != napi::ValueType::Null {
                    values[column] = Some(crate::query_v2_builder_runtime::node_scalar(
                        &env,
                        input.value_type(),
                        value,
                    )?);
                }
            }
            bound.push(type_bridge_contract::query_plan::InputRow::new(values));
        }
        let plan = plan
            .with_given_rows(bound, authority.declared())
            .map_err(|error| napi_error(&error))?;
        Ok(Self {
            plans: Arc::new(match self.plans.as_ref() {
                NodePlanProgram::Raw(_) => NodePlanProgram::Raw(Box::new(BuiltPlans::single(plan))),
                NodePlanProgram::Models(model) => {
                    NodePlanProgram::Models(Box::new(model.with_plans(BuiltPlans::single(plan))?))
                }
            }),
        })
    }

    /// Decode typed scalar columns before JavaScript can round integer evidence.
    #[napi(js_name = "scalarRowsJson")]
    pub fn scalar_rows_json(&self, answer_json: String) -> napi::Result<String> {
        let answer = crate::staged_transaction_runtime::parse_plan_answer_json(&answer_json)?;
        let rows = answer
            .scalar_rows(self.inner()?)
            .map_err(|error| napi_error(&error))?;
        crate::runtime_projection::canonical_plan_scalar_rows_json(&rows)
    }

    /// The canonical bytes, identical in every SDK.
    #[napi(js_name = "canonicalBytes")]
    pub fn canonical_bytes(&self) -> napi::Result<Buffer> {
        self.inner()?
            .canonical_bytes()
            .map(Buffer::from)
            .map_err(|diagnostic| napi_error(&diagnostic))
    }

    /// Canonical explanation JSON; transported input values are omitted.
    #[napi(js_name = "explainJson")]
    pub fn explain_json(&self) -> napi::Result<String> {
        type_bridge_orm::plan_explain::explain(self.inner()?)
            .and_then(|explanation| explanation.canonical_bytes())
            .map(|bytes| String::from_utf8(bytes).expect("canonical JSON is UTF-8"))
            .map_err(|diagnostic| napi_error(&diagnostic))
    }

    /// The plan fingerprint digest in hex.
    #[napi]
    pub fn fingerprint(&self) -> napi::Result<String> {
        self.inner()?
            .fingerprint()
            .map(|fingerprint| fingerprint.as_fingerprint().digest().to_hex())
            .map_err(|diagnostic| napi_error(&diagnostic))
    }

    /// Whether the plan writes, which makes an inferred kind `write`.
    #[napi]
    pub fn writes(&self) -> bool {
        self.plans.writes()
    }
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
        "/tests/fixtures/plan-builder-purchase.json"
    );

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The TypeScript golden: the Rust SDK's hand-built purchase under the
    /// purchase package authority. The authority a `QueryV2Authority` over
    /// the same declared bytes, scope and profile resolves must carry the
    /// same managed semantics, or TypeScript could not reach these bytes.
    /// `TB_UPDATE_PLAN_BUILDER_FIXTURE=1` rewrites the fixture.
    #[test]
    fn typescript_purchase_fixture_is_the_rust_hand_built_purchase() {
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
            std::fs::write(FIXTURE, &expected).unwrap();
        }
        assert_eq!(std::fs::read_to_string(FIXTURE).unwrap(), expected);
    }
}
