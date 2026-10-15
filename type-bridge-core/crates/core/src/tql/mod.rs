//! The official AST is an implementation detail, including at sibling boundaries.
#![allow(missing_docs)]

mod builders;
mod render_guard;
pub use builders::*;

/// A grammar construct whose upstream renderer is incomplete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedConstruct {
    StructDestructuring,
    ExplicitEnd,
    SchemaRename,
}

/// Stable adapter errors contain neither upstream types nor query values.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TqlError {
    InvalidSyntax { line: usize, column: usize },
    InvalidIdentifier,
    InvalidLiteral,
    InvalidOperator,
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
            Self::InvalidOperator => f.write_str("invalid TypeQL operator"),
            Self::Unsupported { .. } => f.write_str("unsupported TypeQL rendering construct"),
        }
    }
}
impl std::error::Error for TqlError {}

#[derive(Debug)]
pub struct Query {
    inner: typeql::Query,
    render_problem: Option<UnsupportedConstruct>,
}

impl Query {
    fn from_ast(mut inner: typeql::Query) -> Self {
        let render_problem = render_guard::prepare(&mut inner);
        Self {
            inner,
            render_problem,
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
    Ok(query.inner.to_string())
}
