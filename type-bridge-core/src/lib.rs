pub mod ast;
pub mod core;

use pyo3::prelude::*;

#[pyclass]
pub struct ValidationEngine {
    inner: core::validation::ValidationEngine,
}

#[pymethods]
impl ValidationEngine {
    #[new]
    fn new() -> Self {
        ValidationEngine {
            inner: core::validation::ValidationEngine::new(),
        }
    }

    // Placeholder for validate method that takes a Python AST node
    fn validate(&self, _node: PyObject) -> PyResult<bool> {
        Ok(true)
    }
}

#[pymodule]
fn type_bridge_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ValidationEngine>()?;

    // Values
    m.add_class::<ast::LiteralValue>()?;
    m.add_class::<ast::FunctionCallValue>()?;
    m.add_class::<ast::ArithmeticValue>()?;
    m.add_class::<ast::RolePlayer>()?;

    // Constraints
    m.add_class::<ast::IidConstraint>()?;
    m.add_class::<ast::HasConstraint>()?;
    m.add_class::<ast::IsaConstraint>()?;

    // Patterns
    m.add_class::<ast::EntityPattern>()?;
    m.add_class::<ast::RelationPattern>()?;
    m.add_class::<ast::SubTypePattern>()?;
    m.add_class::<ast::AttributePattern>()?;
    m.add_class::<ast::HasPattern>()?;
    m.add_class::<ast::ValueComparisonPattern>()?;
    m.add_class::<ast::NotPattern>()?;
    m.add_class::<ast::OrPattern>()?;
    m.add_class::<ast::IidPattern>()?;
    m.add_class::<ast::RawPattern>()?;

    // Statements
    m.add_class::<ast::HasStatement>()?;
    m.add_class::<ast::IsaStatement>()?;
    m.add_class::<ast::RelationStatement>()?;
    m.add_class::<ast::DeleteThingStatement>()?;
    m.add_class::<ast::RawStatement>()?;

    // Clauses
    m.add_class::<ast::MatchClause>()?;
    m.add_class::<ast::MatchLetClause>()?;
    m.add_class::<ast::LetAssignment>()?;
    m.add_class::<ast::InsertClause>()?;
    m.add_class::<ast::DeleteClause>()?;
    m.add_class::<ast::UpdateClause>()?;

    // Fetch Items
    m.add_class::<ast::FetchAttribute>()?;
    m.add_class::<ast::FetchVariable>()?;
    m.add_class::<ast::FetchAttributeList>()?;
    m.add_class::<ast::FetchFunction>()?;
    m.add_class::<ast::FetchWildcard>()?;
    m.add_class::<ast::FetchNestedWildcard>()?;
    m.add_class::<ast::FetchClause>()?;

    // Aggregations
    m.add_class::<ast::AggregateExpr>()?;
    m.add_class::<ast::ReduceAssignment>()?;
    m.add_class::<ast::ReduceClause>()?;

    Ok(())
}
