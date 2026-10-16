//! The official AST is an implementation detail, including at sibling boundaries.
#![allow(missing_docs)]

mod builders;
mod schema_builders;
mod schema_projection;
pub use schema_builders::*;
mod legacy;
mod legacy_projection;
pub use legacy::*;
pub use legacy_projection::project_legacy;
mod render_guard;
pub use builders::*;

/// A grammar construct unsupported by rendering or legacy projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedConstruct {
    StructDestructuring,
    ExplicitEnd,
    SchemaRename,
    RedefineSequence,
    LegacyProjection,
}

/// Stable adapter errors contain neither upstream types nor query values.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TqlError {
    InvalidSyntax { line: usize, column: usize },
    InvalidIdentifier,
    InvalidLiteral,
    InvalidOperator,
    InvalidLegacyShape,
    InvalidSchemaShape,
    Unsupported { construct: UnsupportedConstruct },
}

impl std::fmt::Display for TqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSyntax { line, column } => {
                write!(f, "invalid TypeQL syntax at {line}:{column}")
            }
            Self::InvalidIdentifier => f.write_str("invalid TypeQL identifier"),
            Self::InvalidLiteral => f.write_str("invalid TypeQL literal"),
            Self::InvalidLegacyShape => f.write_str("invalid legacy TypeQL node shape"),
            Self::InvalidSchemaShape => f.write_str("invalid TypeQL schema node shape"),
            Self::InvalidOperator => f.write_str("invalid TypeQL operator"),
            Self::Unsupported { .. } => f.write_str("unsupported TypeQL construct"),
        }
    }
}
impl std::error::Error for TqlError {}

#[derive(Debug)]
pub struct Query {
    inner: typeql::Query,
    render_problem: Option<UnsupportedConstruct>,
    function_bodies: Vec<(usize, String)>,
}

impl Query {
    fn from_ast(mut inner: typeql::Query) -> Self {
        let render_problem = render_guard::prepare(&mut inner);
        Self {
            inner,
            render_problem,
            function_bodies: vec![],
        }
    }
}

pub fn parse(source: &str) -> Result<Query, TqlError> {
    typeql::parse_query(source)
        .map(Query::from_ast)
        .map_err(|error| {
            let (line, column) = error
                .errors()
                .iter()
                .find_map(|error| {
                    if let typeql::common::error::TypeQLError::SyntaxErrorDetailed {
                        error_line_nr,
                        error_col,
                        ..
                    } = error
                    {
                        Some((*error_line_nr, error_col.saturating_add(1)))
                    } else {
                        None
                    }
                })
                .unwrap_or((1, 1));
            TqlError::InvalidSyntax { line, column }
        })
}

pub fn render(query: &Query) -> Result<String, TqlError> {
    if let Some(construct) = query.render_problem {
        return Err(TqlError::Unsupported { construct });
    }
    // Pretty renders Optional as negation in typeql 3.13.4; plain Display is correct.
    let source = query.inner.to_string();
    schema_builders::restore_function_bodies(source, &query.function_bodies)
}

pub use crate::parser::parse_schema;

pub(crate) fn project_schema(
    source: &str,
) -> Result<crate::_schema::TypeSchema, crate::_schema::SchemaError> {
    schema_projection::project(source)
}

/// Compare official ASTs after canonical rendering normalizes source spans.
pub fn equivalent(left: &str, right: &str) -> Result<bool, TqlError> {
    let left = render(&parse(left)?)?;
    let right = render(&parse(right)?)?;
    Ok(parse(&left)?.inner == parse(&right)?.inner)
}
