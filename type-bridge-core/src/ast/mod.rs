use pyo3::prelude::*;
use crate::core;
use pythonize::depythonize;

#[pyclass(get_all, set_all)]
pub struct LiteralValue {
    pub value: PyObject,
    pub value_type: String,
}

#[pymethods]
impl LiteralValue {
    #[new]
    fn new(value: PyObject, value_type: String) -> Self {
        LiteralValue { value, value_type }
    }
}

#[pyclass(get_all, set_all)]
pub struct FunctionCallValue {
    pub function: String,
    pub args: Vec<PyObject>,
}

#[pymethods]
impl FunctionCallValue {
    #[new]
    fn new(function: String, args: Vec<PyObject>) -> Self {
        FunctionCallValue { function, args }
    }
}

#[pyclass(get_all, set_all)]
pub struct ArithmeticValue {
    pub left: PyObject,
    pub operator: String,
    pub right: PyObject,
}

#[pymethods]
impl ArithmeticValue {
    #[new]
    fn new(left: PyObject, operator: String, right: PyObject) -> Self {
        ArithmeticValue { left, operator, right }
    }
}

#[pyclass(get_all, set_all)]
pub struct RolePlayer {
    pub role: String,
    pub player_var: String,
}

#[pymethods]
impl RolePlayer {
    #[new]
    fn new(role: String, player_var: String) -> Self {
        RolePlayer { role, player_var }
    }
}

// Constraints
#[pyclass(get_all, set_all)]
pub struct IidConstraint {
    pub iid: String,
}

#[pymethods]
impl IidConstraint {
    #[new]
    fn new(iid: String) -> Self {
        IidConstraint { iid }
    }
}

#[pyclass(get_all, set_all)]
pub struct HasConstraint {
    pub attr_name: String,
    pub value: PyObject,
}

#[pymethods]
impl HasConstraint {
    #[new]
    fn new(attr_name: String, value: PyObject) -> Self {
        HasConstraint { attr_name, value }
    }
}

#[pyclass(get_all, set_all)]
pub struct IsaConstraint {
    pub type_name: String,
    pub strict: bool,
}

#[pymethods]
impl IsaConstraint {
    #[new]
    #[pyo3(signature = (type_name, strict=false))]
    fn new(type_name: String, strict: bool) -> Self {
        IsaConstraint { type_name, strict }
    }
}

// Patterns
#[pyclass(get_all, set_all)]
pub struct EntityPattern {
    pub variable: String,
    pub type_name: String,
    pub constraints: Vec<PyObject>,
    pub is_strict: bool,
}

#[pymethods]
impl EntityPattern {
    #[new]
    #[pyo3(signature = (variable, type_name, constraints=Vec::new(), is_strict=false))]
    fn new(variable: String, type_name: String, constraints: Vec<PyObject>, is_strict: bool) -> Self {
        EntityPattern {
            variable,
            type_name,
            constraints,
            is_strict,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct RelationPattern {
    pub variable: String,
    pub type_name: String,
    pub role_players: Vec<PyObject>,
    pub constraints: Vec<PyObject>,
}

#[pymethods]
impl RelationPattern {
    #[new]
    #[pyo3(signature = (variable, type_name, role_players=Vec::new(), constraints=Vec::new()))]
    fn new(variable: String, type_name: String, role_players: Vec<PyObject>, constraints: Vec<PyObject>) -> Self {
        RelationPattern {
            variable,
            type_name,
            role_players,
            constraints,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct SubTypePattern {
    pub variable: String,
    pub parent_type: String,
}

#[pymethods]
impl SubTypePattern {
    #[new]
    fn new(variable: String, parent_type: String) -> Self {
        SubTypePattern { variable, parent_type }
    }
}

#[pyclass(get_all, set_all)]
pub struct AttributePattern {
    pub variable: String,
    pub type_name: String,
    pub value: Option<PyObject>,
}

#[pymethods]
impl AttributePattern {
    #[new]
    #[pyo3(signature = (variable, type_name, value=None))]
    fn new(variable: String, type_name: String, value: Option<PyObject>) -> Self {
        AttributePattern { variable, type_name, value }
    }
}

#[pyclass(get_all, set_all)]
pub struct HasPattern {
    pub thing_var: String,
    pub attr_type: String,
    pub attr_var: String,
}

#[pymethods]
impl HasPattern {
    #[new]
    fn new(thing_var: String, attr_type: String, attr_var: String) -> Self {
        HasPattern { thing_var, attr_type, attr_var }
    }
}

#[pyclass(get_all, set_all)]
pub struct ValueComparisonPattern {
    pub var: String,
    pub operator: String,
    pub value: PyObject,
}

#[pymethods]
impl ValueComparisonPattern {
    #[new]
    fn new(var: String, operator: String, value: PyObject) -> Self {
        ValueComparisonPattern { var, operator, value }
    }
}

#[pyclass(get_all, set_all)]
pub struct NotPattern {
    pub patterns: Vec<PyObject>,
}

#[pymethods]
impl NotPattern {
    #[new]
    fn new(patterns: Vec<PyObject>) -> Self {
        NotPattern { patterns }
    }
}

#[pyclass(get_all, set_all)]
pub struct OrPattern {
    pub alternatives: Vec<Vec<PyObject>>,
}

#[pymethods]
impl OrPattern {
    #[new]
    fn new(alternatives: Vec<Vec<PyObject>>) -> Self {
        OrPattern { alternatives }
    }
}

#[pyclass(get_all, set_all)]
pub struct IidPattern {
    pub variable: String,
    pub iid: String,
}

#[pymethods]
impl IidPattern {
    #[new]
    fn new(variable: String, iid: String) -> Self {
        IidPattern { variable, iid }
    }
}

#[pyclass(get_all, set_all)]
pub struct RawPattern {
    pub content: String,
}

#[pymethods]
impl RawPattern {
    #[new]
    fn new(content: String) -> Self {
        RawPattern { content }
    }
}

// Statements
#[pyclass(get_all, set_all)]
pub struct HasStatement {
    pub subject_var: String,
    pub attr_name: String,
    pub value: PyObject,
}

#[pymethods]
impl HasStatement {
    #[new]
    fn new(subject_var: String, attr_name: String, value: PyObject) -> Self {
        HasStatement { subject_var, attr_name, value }
    }
}

#[pyclass(get_all, set_all)]
pub struct IsaStatement {
    pub variable: String,
    pub type_name: String,
}

#[pymethods]
impl IsaStatement {
    #[new]
    fn new(variable: String, type_name: String) -> Self {
        IsaStatement { variable, type_name }
    }
}

#[pyclass(get_all, set_all)]
pub struct RelationStatement {
    pub variable: String,
    pub type_name: String,
    pub role_players: Vec<PyObject>,
    pub include_variable: bool,
    pub attributes: Vec<PyObject>,
}

#[pymethods]
impl RelationStatement {
    #[new]
    #[pyo3(signature = (variable, type_name, role_players=Vec::new(), include_variable=true, attributes=Vec::new()))]
    fn new(variable: String, type_name: String, role_players: Vec<PyObject>, include_variable: bool, attributes: Vec<PyObject>) -> Self {
        RelationStatement {
            variable,
            type_name,
            role_players,
            include_variable,
            attributes,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct DeleteThingStatement {
    pub variable: String,
}

#[pymethods]
impl DeleteThingStatement {
    #[new]
    fn new(variable: String) -> Self {
        DeleteThingStatement { variable }
    }
}

#[pyclass(get_all, set_all)]
pub struct RawStatement {
    pub content: String,
}

#[pymethods]
impl RawStatement {
    #[new]
    fn new(content: String) -> Self {
        RawStatement { content }
    }
}

// Clauses
#[pyclass(get_all, set_all)]
pub struct MatchClause {
    pub patterns: Vec<PyObject>,
}

#[pymethods]
impl MatchClause {
    #[new]
    fn new(patterns: Vec<PyObject>) -> Self {
        MatchClause { patterns }
    }
}

#[pyclass(get_all, set_all)]
pub struct MatchLetClause {
    pub assignments: Vec<PyObject>,
}

#[pymethods]
impl MatchLetClause {
    #[new]
    fn new(assignments: Vec<PyObject>) -> Self {
        MatchLetClause { assignments }
    }
}

#[pyclass(get_all, set_all)]
pub struct LetAssignment {
    pub variables: Vec<String>,
    pub expression: PyObject,
    pub is_stream: bool,
}

#[pymethods]
impl LetAssignment {
    #[new]
    #[pyo3(signature = (variables, expression, is_stream=false))]
    fn new(variables: Vec<String>, expression: PyObject, is_stream: bool) -> Self {
        LetAssignment { variables, expression, is_stream }
    }
}

#[pyclass(get_all, set_all)]
pub struct InsertClause {
    pub statements: Vec<PyObject>,
}

#[pymethods]
impl InsertClause {
    #[new]
    fn new(statements: Vec<PyObject>) -> Self {
        InsertClause { statements }
    }
}

#[pyclass(get_all, set_all)]
pub struct DeleteClause {
    pub statements: Vec<PyObject>,
}

#[pymethods]
impl DeleteClause {
    #[new]
    fn new(statements: Vec<PyObject>) -> Self {
        DeleteClause { statements }
    }
}

#[pyclass(get_all, set_all)]
pub struct UpdateClause {
    pub statements: Vec<PyObject>,
}

#[pymethods]
impl UpdateClause {
    #[new]
    fn new(statements: Vec<PyObject>) -> Self {
        UpdateClause { statements }
    }
}

// Fetch Items
#[pyclass(get_all, set_all)]
pub struct FetchAttribute {
    pub key: String,
    pub var: String,
    pub attr_name: String,
}

#[pymethods]
impl FetchAttribute {
    #[new]
    fn new(key: String, var: String, attr_name: String) -> Self {
        FetchAttribute { key, var, attr_name }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchVariable {
    pub key: String,
    pub var: String,
}

#[pymethods]
impl FetchVariable {
    #[new]
    fn new(key: String, var: String) -> Self {
        FetchVariable { key, var }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchAttributeList {
    pub key: String,
    pub var: String,
    pub attr_name: String,
}

#[pymethods]
impl FetchAttributeList {
    #[new]
    fn new(key: String, var: String, attr_name: String) -> Self {
        FetchAttributeList { key, var, attr_name }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchFunction {
    pub key: String,
    pub func_name: String,
    pub var: String,
}

#[pymethods]
impl FetchFunction {
    #[new]
    fn new(key: String, func_name: String, var: String) -> Self {
        FetchFunction { key, func_name, var }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchWildcard {
    pub key: String,
    pub var: String,
}

#[pymethods]
impl FetchWildcard {
    #[new]
    fn new(key: String, var: String) -> Self {
        FetchWildcard { key, var }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchNestedWildcard {
    pub key: String,
    pub var: String,
}

#[pymethods]
impl FetchNestedWildcard {
    #[new]
    fn new(key: String, var: String) -> Self {
        FetchNestedWildcard { key, var }
    }
}

#[pyclass(get_all, set_all)]
pub struct FetchClause {
    pub items: Vec<PyObject>,
}

#[pymethods]
impl FetchClause {
    #[new]
    fn new(items: Vec<PyObject>) -> Self {
        FetchClause { items }
    }
}

// Aggregations
#[pyclass(get_all, set_all)]
pub struct AggregateExpr {
    pub func_name: String,
    pub var: String,
    pub attr_name: Option<String>,
}

#[pymethods]
impl AggregateExpr {
    #[new]
    #[pyo3(signature = (func_name, var, attr_name=None))]
    fn new(func_name: String, var: String, attr_name: Option<String>) -> Self {
        AggregateExpr { func_name, var, attr_name }
    }
}

#[pyclass(get_all, set_all)]
pub struct ReduceAssignment {
    pub variable: String,
    pub expression: PyObject,
}

#[pymethods]
impl ReduceAssignment {
    #[new]
    fn new(variable: String, expression: PyObject) -> Self {
        ReduceAssignment { variable, expression }
    }
}

#[pyclass(get_all, set_all)]
pub struct ReduceClause {
    pub assignments: Vec<PyObject>,
    pub group_by: Option<String>,
}

#[pymethods]
impl ReduceClause {
    #[new]
    #[pyo3(signature = (assignments, group_by=None))]
    fn new(assignments: Vec<PyObject>, group_by: Option<String>) -> Self {
        ReduceClause { assignments, group_by }
    }
}

// Conversion Logic
impl FromPyObject<'_> for core::ast::Value {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        if let Ok(lit) = ob.downcast::<LiteralValue>() {
            let lit = lit.borrow();
            let json_val = depythonize(&lit.value.bind(py))?;
            Ok(core::ast::Value::Literal(core::ast::LiteralValue {
                value: json_val,
                value_type: lit.value_type.clone(),
            }))
        } else if let Ok(var) = ob.extract::<String>() {
            Ok(core::ast::Value::Variable(var))
        } else if let Ok(func) = ob.downcast::<FunctionCallValue>() {
            let func = func.borrow();
            let mut args = Vec::new();
            for arg in &func.args {
                args.push(arg.bind(py).extract::<core::ast::Value>()?);
            }
            Ok(core::ast::Value::FunctionCall(core::ast::FunctionCallValue {
                function: func.function.clone(),
                args,
            }))
        } else if let Ok(arith) = ob.downcast::<ArithmeticValue>() {
            let arith = arith.borrow();
            Ok(core::ast::Value::Arithmetic(core::ast::ArithmeticValue {
                left: Box::new(arith.left.bind(py).extract::<core::ast::Value>()?),
                operator: arith.operator.clone(),
                right: Box::new(arith.right.bind(py).extract::<core::ast::Value>()?),
            }))
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected Value (Literal, Variable, FunctionCall, or Arithmetic)"))
        }
    }
}

impl FromPyObject<'_> for core::ast::Constraint {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        if let Ok(iid) = ob.downcast::<IidConstraint>() {
            let iid = iid.borrow();
            Ok(core::ast::Constraint::Iid(iid.iid.clone()))
        } else if let Ok(has) = ob.downcast::<HasConstraint>() {
            let has = has.borrow();
            Ok(core::ast::Constraint::Has {
                attr_name: has.attr_name.clone(),
                value: has.value.bind(py).extract::<core::ast::Value>()?,
            })
        } else if let Ok(isa) = ob.downcast::<IsaConstraint>() {
            let isa = isa.borrow();
            Ok(core::ast::Constraint::Isa {
                type_name: isa.type_name.clone(),
                strict: isa.strict,
            })
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected Constraint"))
        }
    }
}

impl FromPyObject<'_> for core::ast::Pattern {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        if let Ok(ent) = ob.downcast::<EntityPattern>() {
            let ent = ent.borrow();
            let mut constraints = Vec::new();
            for c in &ent.constraints {
                constraints.push(c.bind(py).extract::<core::ast::Constraint>()?);
            }
            Ok(core::ast::Pattern::Entity {
                variable: ent.variable.clone(),
                type_name: ent.type_name.clone(),
                constraints,
                is_strict: ent.is_strict,
            })
        } else if let Ok(rel) = ob.downcast::<RelationPattern>() {
            let rel = rel.borrow();
            let mut role_players = Vec::new();
            for rp in &rel.role_players {
                // rp is &PyObject, need to bind it
                let rp_bound = rp.bind(py);
                let rp_rust = rp_bound.downcast::<RolePlayer>()?.borrow();
                role_players.push(core::ast::RolePlayer {
                    role: rp_rust.role.clone(),
                    player_var: rp_rust.player_var.clone(),
                });
            }
            let mut constraints = Vec::new();
            for c in &rel.constraints {
                constraints.push(c.bind(py).extract::<core::ast::Constraint>()?);
            }
            Ok(core::ast::Pattern::Relation {
                variable: rel.variable.clone(),
                type_name: rel.type_name.clone(),
                role_players,
                constraints,
            })
        } else if let Ok(sub) = ob.downcast::<SubTypePattern>() {
            let sub = sub.borrow();
            Ok(core::ast::Pattern::SubType {
                variable: sub.variable.clone(),
                parent_type: sub.parent_type.clone(),
            })
        } else if let Ok(has) = ob.downcast::<HasPattern>() {
            let has = has.borrow();
            Ok(core::ast::Pattern::Has {
                thing_var: has.thing_var.clone(),
                attr_type: has.attr_type.clone(),
                attr_var: has.attr_var.clone(),
            })
        } else if let Ok(comp) = ob.downcast::<ValueComparisonPattern>() {
            let comp = comp.borrow();
            Ok(core::ast::Pattern::ValueComparison {
                var: comp.var.clone(),
                operator: comp.operator.clone(),
                value: comp.value.bind(py).extract::<core::ast::Value>()?,
            })
        } else if let Ok(not) = ob.downcast::<NotPattern>() {
            let not = not.borrow();
            let mut patterns = Vec::new();
            for p in &not.patterns {
                patterns.push(p.bind(py).extract::<core::ast::Pattern>()?);
            }
            Ok(core::ast::Pattern::Not(patterns))
        } else if let Ok(or) = ob.downcast::<OrPattern>() {
            let or = or.borrow();
            let mut alternatives = Vec::new();
            for alt in &or.alternatives {
                let mut alt_rust = Vec::new();
                for p in alt {
                    alt_rust.push(p.bind(py).extract::<core::ast::Pattern>()?);
                }
                alternatives.push(alt_rust);
            }
            Ok(core::ast::Pattern::Or(alternatives))
        } else if let Ok(iid) = ob.downcast::<IidPattern>() {
            let iid = iid.borrow();
            Ok(core::ast::Pattern::Iid {
                variable: iid.variable.clone(),
                iid: iid.iid.clone(),
            })
        } else if let Ok(attr) = ob.downcast::<AttributePattern>() {
            let attr = attr.borrow();
            let value = match &attr.value {
                Some(v) => Some(v.bind(py).extract::<core::ast::Value>()?),
                None => None,
            };
            Ok(core::ast::Pattern::Attribute {
                variable: attr.variable.clone(),
                type_name: attr.type_name.clone(),
                value,
            })
        } else if let Ok(raw) = ob.downcast::<RawPattern>() {
            let raw = raw.borrow();
            Ok(core::ast::Pattern::Raw(raw.content.clone()))
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected Pattern"))
        }
    }
}

impl FromPyObject<'_> for core::ast::Statement {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        if let Ok(has) = ob.downcast::<HasStatement>() {
            let has = has.borrow();
            Ok(core::ast::Statement::Has {
                subject_var: has.subject_var.clone(),
                attr_name: has.attr_name.clone(),
                value: has.value.bind(py).extract::<core::ast::Value>()?,
            })
        } else if let Ok(isa) = ob.downcast::<IsaStatement>() {
            let isa = isa.borrow();
            Ok(core::ast::Statement::Isa {
                variable: isa.variable.clone(),
                type_name: isa.type_name.clone(),
            })
        } else if let Ok(rel) = ob.downcast::<RelationStatement>() {
            let rel = rel.borrow();
            let mut role_players = Vec::new();
            for rp in &rel.role_players {
                let rp_rust = rp.bind(py).downcast::<RolePlayer>()?.borrow();
                role_players.push(core::ast::RolePlayer {
                    role: rp_rust.role.clone(),
                    player_var: rp_rust.player_var.clone(),
                });
            }
            let mut attributes = Vec::new();
            for attr in &rel.attributes {
                attributes.push(attr.bind(py).extract::<core::ast::Statement>()?);
            }
            Ok(core::ast::Statement::Relation {
                variable: rel.variable.clone(),
                type_name: rel.type_name.clone(),
                role_players,
                include_variable: rel.include_variable,
                attributes,
            })
        } else if let Ok(del) = ob.downcast::<DeleteThingStatement>() {
            let del = del.borrow();
            Ok(core::ast::Statement::DeleteThing(del.variable.clone()))
        } else if let Ok(raw) = ob.downcast::<RawStatement>() {
            let raw = raw.borrow();
            Ok(core::ast::Statement::Raw(raw.content.clone()))
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected Statement"))
        }
    }
}

impl FromPyObject<'_> for core::ast::Clause {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        if let Ok(m) = ob.downcast::<MatchClause>() {
            let m = m.borrow();
            let mut patterns = Vec::new();
            for p in &m.patterns {
                patterns.push(p.bind(py).extract::<core::ast::Pattern>()?);
            }
            Ok(core::ast::Clause::Match(patterns))
        } else if let Ok(ins) = ob.downcast::<InsertClause>() {
            let ins = ins.borrow();
            let mut statements = Vec::new();
            for s in &ins.statements {
                statements.push(s.bind(py).extract::<core::ast::Statement>()?);
            }
            Ok(core::ast::Clause::Insert(statements))
        } else if let Ok(del) = ob.downcast::<DeleteClause>() {
            let del = del.borrow();
            let mut statements = Vec::new();
            for s in &del.statements {
                statements.push(s.bind(py).extract::<core::ast::Statement>()?);
            }
            Ok(core::ast::Clause::Delete(statements))
        } else if let Ok(upd) = ob.downcast::<UpdateClause>() {
            let upd = upd.borrow();
            let mut statements = Vec::new();
            for s in &upd.statements {
                statements.push(s.bind(py).extract::<core::ast::Statement>()?);
            }
            Ok(core::ast::Clause::Update(statements))
        } else if let Ok(f) = ob.downcast::<FetchClause>() {
            let f = f.borrow();
            let mut items = Vec::new();
            for item in &f.items {
                items.push(item.bind(py).extract::<core::ast::FetchItem>()?);
            }
            Ok(core::ast::Clause::Fetch(items))
        } else if let Ok(ml) = ob.downcast::<MatchLetClause>() {
            let ml = ml.borrow();
            let mut assignments = Vec::new();
            for a in &ml.assignments {
                assignments.push(a.bind(py).extract::<core::ast::LetAssignment>()?);
            }
            Ok(core::ast::Clause::MatchLet(assignments))
        } else if let Ok(r) = ob.downcast::<ReduceClause>() {
            let r = r.borrow();
            let mut assignments = Vec::new();
            for a in &r.assignments {
                assignments.push(a.bind(py).extract::<core::ast::ReduceAssignment>()?);
            }
            Ok(core::ast::Clause::Reduce {
                assignments,
                group_by: r.group_by.clone(),
            })
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected Clause"))
        }
    }
}

impl FromPyObject<'_> for core::ast::FetchItem {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(attr) = ob.downcast::<FetchAttribute>() {
            let attr = attr.borrow();
            Ok(core::ast::FetchItem::Attribute { key: attr.key.clone(), var: attr.var.clone(), attr_name: attr.attr_name.clone() })
        } else if let Ok(var) = ob.downcast::<FetchVariable>() {
            let var = var.borrow();
            Ok(core::ast::FetchItem::Variable { key: var.key.clone(), var: var.var.clone() })
        } else if let Ok(list) = ob.downcast::<FetchAttributeList>() {
            let list = list.borrow();
            Ok(core::ast::FetchItem::AttributeList { key: list.key.clone(), var: list.var.clone(), attr_name: list.attr_name.clone() })
        } else if let Ok(func) = ob.downcast::<FetchFunction>() {
            let func = func.borrow();
            Ok(core::ast::FetchItem::Function { key: func.key.clone(), func_name: func.func_name.clone(), var: func.var.clone() })
        } else if let Ok(wild) = ob.downcast::<FetchWildcard>() {
            let wild = wild.borrow();
            Ok(core::ast::FetchItem::Wildcard { key: wild.key.clone(), var: wild.var.clone() })
        } else if let Ok(nested) = ob.downcast::<FetchNestedWildcard>() {
            let nested = nested.borrow();
            Ok(core::ast::FetchItem::NestedWildcard { key: nested.key.clone(), var: nested.var.clone() })
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Expected FetchItem"))
        }
    }
}

impl FromPyObject<'_> for core::ast::LetAssignment {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        let assign = ob.downcast::<LetAssignment>()?.borrow();
        Ok(core::ast::LetAssignment {
            variables: assign.variables.clone(),
            expression: assign.expression.bind(py).extract::<core::ast::Value>()?,
            is_stream: assign.is_stream,
        })
    }
}

impl FromPyObject<'_> for core::ast::ReduceAssignment {
    fn extract_bound(ob: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = ob.py();
        let assign = ob.downcast::<ReduceAssignment>()?.borrow();
        Ok(core::ast::ReduceAssignment {
            variable: assign.variable.clone(),
            expression: assign.expression.bind(py).extract::<core::ast::Value>()?,
        })
    }
}