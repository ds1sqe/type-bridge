//! PyO3 wrapper types that mirror [`type_bridge_core_lib::ast`].
//!
//! These are 1:1 Python-facing re-exports of the core AST structs and are
//! intentionally undocumented — see the core crate for semantics.

#![allow(missing_docs)]

use pyo3::prelude::*;

#[pyclass(get_all, set_all)]
pub struct LiteralValue {
    pub value: Py<PyAny>,
    pub value_type: String,
}

#[pymethods]
impl LiteralValue {
    #[new]
    fn new(value: Py<PyAny>, value_type: String) -> Self {
        LiteralValue { value, value_type }
    }
}

#[pyclass(get_all, set_all)]
pub struct FunctionCallValue {
    pub function: String,
    pub args: Vec<Py<PyAny>>,
}

#[pymethods]
impl FunctionCallValue {
    #[new]
    fn new(function: String, args: Vec<Py<PyAny>>) -> Self {
        FunctionCallValue { function, args }
    }
}

#[pyclass(get_all, set_all)]
pub struct ArithmeticValue {
    pub left: Py<PyAny>,
    pub operator: String,
    pub right: Py<PyAny>,
}

#[pymethods]
impl ArithmeticValue {
    #[new]
    fn new(left: Py<PyAny>, operator: String, right: Py<PyAny>) -> Self {
        ArithmeticValue {
            left,
            operator,
            right,
        }
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
    pub value: Py<PyAny>,
}

#[pymethods]
impl HasConstraint {
    #[new]
    fn new(attr_name: String, value: Py<PyAny>) -> Self {
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
    pub constraints: Vec<Py<PyAny>>,
    pub is_strict: bool,
}

#[pymethods]
impl EntityPattern {
    #[new]
    #[pyo3(signature = (variable, type_name, constraints=Vec::new(), is_strict=false))]
    fn new(
        variable: String,
        type_name: String,
        constraints: Vec<Py<PyAny>>,
        is_strict: bool,
    ) -> Self {
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
    pub role_players: Vec<Py<PyAny>>,
    pub constraints: Vec<Py<PyAny>>,
}

#[pymethods]
impl RelationPattern {
    #[new]
    #[pyo3(signature = (variable, type_name, role_players=Vec::new(), constraints=Vec::new()))]
    fn new(
        variable: String,
        type_name: String,
        role_players: Vec<Py<PyAny>>,
        constraints: Vec<Py<PyAny>>,
    ) -> Self {
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
        SubTypePattern {
            variable,
            parent_type,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct AttributePattern {
    pub variable: String,
    pub type_name: String,
    pub value: Option<Py<PyAny>>,
}

#[pymethods]
impl AttributePattern {
    #[new]
    #[pyo3(signature = (variable, type_name, value=None))]
    fn new(variable: String, type_name: String, value: Option<Py<PyAny>>) -> Self {
        AttributePattern {
            variable,
            type_name,
            value,
        }
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
        HasPattern {
            thing_var,
            attr_type,
            attr_var,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct ValueComparisonPattern {
    pub var: String,
    pub operator: String,
    pub value: Py<PyAny>,
}

#[pymethods]
impl ValueComparisonPattern {
    #[new]
    fn new(var: String, operator: String, value: Py<PyAny>) -> Self {
        ValueComparisonPattern {
            var,
            operator,
            value,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct NotPattern {
    pub patterns: Vec<Py<PyAny>>,
}

#[pymethods]
impl NotPattern {
    #[new]
    fn new(patterns: Vec<Py<PyAny>>) -> Self {
        NotPattern { patterns }
    }
}

#[pyclass(get_all, set_all)]
pub struct OrPattern {
    pub alternatives: Vec<Vec<Py<PyAny>>>,
}

#[pymethods]
impl OrPattern {
    #[new]
    fn new(alternatives: Vec<Vec<Py<PyAny>>>) -> Self {
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
    pub value: Py<PyAny>,
}

#[pymethods]
impl HasStatement {
    #[new]
    fn new(subject_var: String, attr_name: String, value: Py<PyAny>) -> Self {
        HasStatement {
            subject_var,
            attr_name,
            value,
        }
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
        IsaStatement {
            variable,
            type_name,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct RelationStatement {
    pub variable: String,
    pub type_name: String,
    pub role_players: Vec<Py<PyAny>>,
    pub include_variable: bool,
    pub attributes: Vec<Py<PyAny>>,
}

#[pymethods]
impl RelationStatement {
    #[new]
    #[pyo3(signature = (variable, type_name, role_players=Vec::new(), include_variable=true, attributes=Vec::new()))]
    fn new(
        variable: String,
        type_name: String,
        role_players: Vec<Py<PyAny>>,
        include_variable: bool,
        attributes: Vec<Py<PyAny>>,
    ) -> Self {
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
    pub patterns: Vec<Py<PyAny>>,
}

#[pymethods]
impl MatchClause {
    #[new]
    fn new(patterns: Vec<Py<PyAny>>) -> Self {
        MatchClause { patterns }
    }
}

#[pyclass(get_all, set_all)]
pub struct MatchLetClause {
    pub assignments: Vec<Py<PyAny>>,
}

#[pymethods]
impl MatchLetClause {
    #[new]
    fn new(assignments: Vec<Py<PyAny>>) -> Self {
        MatchLetClause { assignments }
    }
}

#[pyclass(get_all, set_all)]
pub struct LetAssignment {
    pub variables: Vec<String>,
    pub expression: Py<PyAny>,
    pub is_stream: bool,
}

#[pymethods]
impl LetAssignment {
    #[new]
    #[pyo3(signature = (variables, expression, is_stream=false))]
    fn new(variables: Vec<String>, expression: Py<PyAny>, is_stream: bool) -> Self {
        LetAssignment {
            variables,
            expression,
            is_stream,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct InsertClause {
    pub statements: Vec<Py<PyAny>>,
}

#[pymethods]
impl InsertClause {
    #[new]
    fn new(statements: Vec<Py<PyAny>>) -> Self {
        InsertClause { statements }
    }
}

#[pyclass(get_all, set_all)]
pub struct DeleteClause {
    pub statements: Vec<Py<PyAny>>,
}

#[pymethods]
impl DeleteClause {
    #[new]
    fn new(statements: Vec<Py<PyAny>>) -> Self {
        DeleteClause { statements }
    }
}

#[pyclass(get_all, set_all)]
pub struct UpdateClause {
    pub statements: Vec<Py<PyAny>>,
}

#[pymethods]
impl UpdateClause {
    #[new]
    fn new(statements: Vec<Py<PyAny>>) -> Self {
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
        FetchAttribute {
            key,
            var,
            attr_name,
        }
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
        FetchAttributeList {
            key,
            var,
            attr_name,
        }
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
        FetchFunction {
            key,
            func_name,
            var,
        }
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
    pub items: Vec<Py<PyAny>>,
}

#[pymethods]
impl FetchClause {
    #[new]
    fn new(items: Vec<Py<PyAny>>) -> Self {
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
        AggregateExpr {
            func_name,
            var,
            attr_name,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct ReduceAssignment {
    pub variable: String,
    pub expression: Py<PyAny>,
}

#[pymethods]
impl ReduceAssignment {
    #[new]
    fn new(variable: String, expression: Py<PyAny>) -> Self {
        ReduceAssignment {
            variable,
            expression,
        }
    }
}

#[pyclass(get_all, set_all)]
pub struct ReduceClause {
    pub assignments: Vec<Py<PyAny>>,
    pub group_by: Option<String>,
}

#[pymethods]
impl ReduceClause {
    #[new]
    #[pyo3(signature = (assignments, group_by=None))]
    fn new(assignments: Vec<Py<PyAny>>, group_by: Option<String>) -> Self {
        ReduceClause {
            assignments,
            group_by,
        }
    }
}
