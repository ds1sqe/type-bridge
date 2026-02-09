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

    fn validate_type_name(&self, name: String, context: String) -> PyResult<bool> {
        let result = self.inner.validate_type_name(&name, &context);
        if !result.is_valid {
            return Err(pyo3::exceptions::PyValueError::new_err(result.errors[0].message.clone()));
        }
        Ok(true)
    }

    fn validate_pattern(&self, pattern: Bound<'_, PyAny>) -> PyResult<bool> {
        let core_pattern = pattern.extract::<core::ast::Pattern>()?;
        let result = self.inner.validate_pattern(&core_pattern);
        Ok(result.is_valid)
    }

    fn validate_statement(&self, statement: Bound<'_, PyAny>) -> PyResult<bool> {
        let core_statement = statement.extract::<core::ast::Statement>()?;
        let result = self.inner.validate_statement(&core_statement);
        Ok(result.is_valid)
    }
}

#[pyclass]
pub struct QueryCompiler {
    inner: core::compiler::QueryCompiler,
}

#[pymethods]
impl QueryCompiler {
    #[new]
    fn new() -> Self {
        QueryCompiler {
            inner: core::compiler::QueryCompiler::new(),
        }
    }

    fn compile(&self, clauses: Vec<Bound<'_, PyAny>>) -> PyResult<String> {
        let mut core_clauses = Vec::new();
        for clause in clauses {
            core_clauses.push(clause.extract::<core::ast::Clause>()?);
        }
        Ok(self.inner.compile(&core_clauses))
    }
}

#[pymodule]
fn type_bridge_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ValidationEngine>()?;
    m.add_class::<QueryCompiler>()?;

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
