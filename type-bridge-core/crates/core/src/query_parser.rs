//! Winnow-based parser for TypeQL data-manipulation queries.
//!
//! Converts TypeQL query strings (match, insert, delete, update, fetch, reduce)
//! back into [`Clause`](crate::ast::Clause) AST nodes — the reverse of [`QueryCompiler`](crate::compiler::QueryCompiler).

use std::fmt;

use winnow::combinator::{alt, opt, separated};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{literal, take_while};

use crate::ast::{
    Clause, Constraint, FetchItem, LetAssignment, LiteralValue, Pattern, ReduceAssignment,
    RolePlayer, SortField, Statement, Value,
};

/// Type alias for winnow parser results.
type PResult<T> = winnow::error::Result<T>;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error returned by [`parse_typeql_query`].
#[derive(Debug, Clone)]
pub enum QueryParseError {
    /// The TypeQL query string could not be parsed.
    ParseError {
        /// Human-readable description of the parse failure.
        message: String,
        /// 1-based line number where the error occurred.
        line: usize,
        /// 1-based column number where the error occurred.
        column: usize,
    },
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryParseError::ParseError {
                message,
                line,
                column,
            } => write!(f, "parse error at {}:{}: {}", line, column, message),
        }
    }
}

impl std::error::Error for QueryParseError {}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a TypeQL query string into a list of [`Clause`] AST nodes.
///
/// Handles multi-clause queries (e.g. `match ... insert ...`).
pub fn parse_typeql_query(input: &str) -> Result<Vec<Clause>, QueryParseError> {
    let mut input_ref = input;
    // Handle empty/whitespace-only input gracefully
    ws_comments(&mut input_ref);
    if input_ref.is_empty() {
        return Ok(Vec::new());
    }
    match parse_clauses(&mut input_ref) {
        Ok(clauses) => {
            ws_comments(&mut input_ref);
            if !input_ref.is_empty() {
                let consumed = input.len() - input_ref.len();
                let (line, column) = line_col(input, consumed);
                return Err(QueryParseError::ParseError {
                    message: format!("unexpected trailing content at {}:{}", line, column),
                    line,
                    column,
                });
            }
            Ok(clauses)
        }
        Err(_e) => {
            let consumed = input.len() - input_ref.len();
            let (line, column) = line_col(input, consumed);
            Err(QueryParseError::ParseError {
                message: format!("unexpected input at {}:{}", line, column),
                line,
                column,
            })
        }
    }
}

/// Compute 1-based line and column from a byte offset.
fn line_col(input: &str, offset: usize) -> (usize, usize) {
    let parsed_portion = &input[..offset];
    let line = parsed_portion.chars().filter(|c| *c == '\n').count() + 1;
    let last_newline = parsed_portion.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = offset - last_newline + 1;
    (line, column)
}

// ---------------------------------------------------------------------------
// Whitespace / comment helpers (duplicated from parser.rs to avoid coupling)
// ---------------------------------------------------------------------------

/// Skip whitespace and comments (`#`, `//`, `/* */`).
fn ws_comments(input: &mut &str) {
    loop {
        let before = *input;
        let _: &str = take_while::<_, _, ContextError>(0.., |c: char| c.is_ascii_whitespace())
            .parse_next(input)
            .unwrap();

        if input.starts_with('#') || input.starts_with("//") {
            if let Some(pos) = input.find('\n') {
                *input = &input[pos + 1..];
            } else {
                *input = &input[input.len()..];
            }
            continue;
        }

        if input.starts_with("/*")
            && let Some(pos) = input.find("*/")
        {
            *input = &input[pos + 2..];
            continue;
        }

        if *input == before {
            break;
        }
    }
}

/// Require at least one whitespace character or comment.
fn ws_comments_required(input: &mut &str) -> PResult<()> {
    let before = *input;
    ws_comments(input);
    if *input == before {
        return Err(ContextError::new());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Token parsers
// ---------------------------------------------------------------------------

/// Parse an identifier: `[a-zA-Z_][a-zA-Z0-9_-]*`.
fn identifier<'a>(input: &mut &'a str) -> PResult<&'a str> {
    let ident: &str = take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-'
    })
    .parse_next(input)?;

    let first = ident.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(ContextError::new());
    }

    Ok(ident)
}

/// Parse a variable: `$identifier`.
fn variable(input: &mut &str) -> PResult<String> {
    literal("$").parse_next(input)?;
    let name = identifier(input)?;
    Ok(format!("${}", name))
}

/// Parse a double-quoted string literal with escape handling.
fn quoted_string(input: &mut &str) -> PResult<String> {
    literal("\"").parse_next(input)?;
    let mut result = String::new();
    loop {
        if input.is_empty() {
            return Err(ContextError::new());
        }
        if input.starts_with('"') {
            *input = &input[1..];
            break;
        }
        if input.starts_with('\\') {
            *input = &input[1..];
            if input.is_empty() {
                return Err(ContextError::new());
            }
            let ch = input.chars().next().unwrap();
            match ch {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                '\\' => result.push('\\'),
                '"' => result.push('"'),
                other => {
                    result.push('\\');
                    result.push(other);
                }
            };
            *input = &input[ch.len_utf8()..];
        } else {
            let ch = input.chars().next().unwrap();
            result.push(ch);
            *input = &input[ch.len_utf8()..];
        }
    }
    Ok(result)
}

/// Parse a hex ID: `0x[0-9a-fA-F]+`.
fn hex_id<'a>(input: &mut &'a str) -> PResult<&'a str> {
    let start = *input;
    literal("0x").parse_next(input)?;
    let _digits: &str = take_while(1.., |c: char| c.is_ascii_hexdigit()).parse_next(input)?;
    let consumed = start.len() - input.len();
    Ok(&start[..consumed])
}

// ---------------------------------------------------------------------------
// Value parsers
// ---------------------------------------------------------------------------

/// Parse a value (literal, variable, function call, or arithmetic expression).
fn parse_value(input: &mut &str) -> PResult<Value> {
    alt((
        parse_arithmetic_value,
        parse_function_call_or_variable,
        parse_literal,
    ))
    .parse_next(input)
}

/// Parse a parenthesized arithmetic expression: `(left op right)`.
fn parse_arithmetic_value(input: &mut &str) -> PResult<Value> {
    literal("(").parse_next(input)?;
    ws_comments(input);
    let left = parse_value(input)?;
    ws_comments(input);
    let op = parse_arithmetic_op(input)?;
    ws_comments(input);
    let right = parse_value(input)?;
    ws_comments(input);
    literal(")").parse_next(input)?;
    Ok(Value::Arithmetic(super::ast::ArithmeticValue {
        left: Box::new(left),
        operator: op.to_string(),
        right: Box::new(right),
    }))
}

fn parse_arithmetic_op<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt((
        literal("+"),
        literal("-"),
        literal("*"),
        literal("/"),
        literal("%"),
        literal("^"),
    ))
    .parse_next(input)
}

/// Parse a function call `func(args...)` or a plain variable `$var`.
/// Combined because both start with an identifier-like prefix.
fn parse_function_call_or_variable(input: &mut &str) -> PResult<Value> {
    // Try variable first (starts with $)
    if input.starts_with('$') {
        let var = variable(input)?;
        return Ok(Value::Variable(var));
    }

    // Otherwise try function call: identifier(args)
    let func_name = identifier(input)?;
    ws_comments(input);
    literal("(").parse_next(input)?;
    ws_comments(input);

    let args = if input.starts_with(')') {
        vec![]
    } else {
        let args: Vec<Value> = separated(
            1..,
            |i: &mut &str| {
                ws_comments(i);
                let v = parse_value(i)?;
                ws_comments(i);
                Ok(v)
            },
            ",",
        )
        .parse_next(input)?;
        args
    };

    ws_comments(input);
    literal(")").parse_next(input)?;

    Ok(Value::FunctionCall(super::ast::FunctionCallValue {
        function: func_name.to_string(),
        args,
    }))
}

/// Parse a literal value (string, boolean, decimal, datetime-tz, datetime, date, duration, double, long).
fn parse_literal(input: &mut &str) -> PResult<Value> {
    alt((
        parse_string_literal,
        parse_boolean_literal,
        parse_number_or_date_literal,
        parse_duration_literal,
    ))
    .parse_next(input)
}

/// Parse a string literal: `"..."`.
fn parse_string_literal(input: &mut &str) -> PResult<Value> {
    let s = quoted_string(input)?;
    Ok(Value::Literal(LiteralValue {
        value: serde_json::Value::String(s),
        value_type: "string".to_string(),
    }))
}

/// Parse a boolean literal: `true` or `false`.
fn parse_boolean_literal(input: &mut &str) -> PResult<Value> {
    let val = alt((literal("true"), literal("false"))).parse_next(input)?;
    // Make sure the boolean keyword isn't followed by alphanumeric (e.g., "trueish")
    if let Some(c) = input.chars().next()
        && (c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ContextError::new());
    }
    Ok(Value::Literal(LiteralValue {
        value: serde_json::Value::Bool(val == "true"),
        value_type: "boolean".to_string(),
    }))
}

/// Parse a number literal (long, double, decimal) or a date/datetime literal.
/// These share a leading digit pattern so they must be tried together.
fn parse_number_or_date_literal(input: &mut &str) -> PResult<Value> {
    let start = *input;

    // Check for negative sign
    let negative = opt(literal::<_, _, ContextError>("-"))
        .parse_next(input)?
        .is_some();

    // Parse leading digits
    let digits: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;

    // Check if this looks like a date: YYYY-MM-DD (4 digits followed by -)
    if !negative && digits.len() == 4 && input.starts_with('-') {
        // Try parsing as date/datetime — restore input and parse specifically
        *input = start;
        if let Ok(v) = parse_date_or_datetime(input) {
            return Ok(v);
        }
        // If date parse fails, restore and try as number
        *input = start;
        if negative {
            literal::<_, _, ContextError>("-").parse_next(input).ok();
        }
        let _: &str = take_while::<_, _, ContextError>(1.., |c: char| c.is_ascii_digit())
            .parse_next(input)?;
    }

    // Check for decimal point (double)
    if input.starts_with('.') {
        *input = &input[1..];
        let frac: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;

        // Check for 'dec' suffix (decimal type)
        if input.starts_with("dec") {
            let consumed = start.len() - input.len();
            let num_str = &start[..consumed];
            *input = &input[3..];
            // Strip "dec" wasn't consumed above, but the number string is clean
            return Ok(Value::Literal(LiteralValue {
                value: serde_json::json!(num_str.parse::<f64>().unwrap_or(0.0)),
                value_type: "decimal".to_string(),
            }));
        }

        // Regular double
        let num_str = if negative {
            format!("-{}.{}", digits, frac)
        } else {
            format!("{}.{}", digits, frac)
        };
        let f: f64 = num_str.parse().map_err(|_| ContextError::new())?;
        return Ok(Value::Literal(LiteralValue {
            value: serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            value_type: "double".to_string(),
        }));
    }

    // Check for 'dec' suffix on integer (decimal type)
    if input.starts_with("dec") {
        *input = &input[3..];
        let num_str = if negative {
            format!("-{}", digits)
        } else {
            digits.to_string()
        };
        let n: i64 = num_str.parse().map_err(|_| ContextError::new())?;
        return Ok(Value::Literal(LiteralValue {
            value: serde_json::json!(n),
            value_type: "decimal".to_string(),
        }));
    }

    // Plain integer (long)
    let num_str = if negative {
        format!("-{}", digits)
    } else {
        digits.to_string()
    };
    let n: i64 = num_str.parse().map_err(|_| ContextError::new())?;
    Ok(Value::Literal(LiteralValue {
        value: serde_json::json!(n),
        value_type: "long".to_string(),
    }))
}

/// Parse a date or datetime literal (unquoted in TypeQL).
/// Formats: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SS[.fff]`, `YYYY-MM-DDTHH:MM:SS[.fff]+HH:MM`.
fn parse_date_or_datetime(input: &mut &str) -> PResult<Value> {
    let start = *input;

    // YYYY-MM-DD
    let _year: &str = take_while(4..=4, |c: char| c.is_ascii_digit()).parse_next(input)?;
    literal("-").parse_next(input)?;
    let _month: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;
    literal("-").parse_next(input)?;
    let _day: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;

    // Check for time component
    if !input.starts_with('T') {
        let consumed = start.len() - input.len();
        let date_str = &start[..consumed];
        return Ok(Value::Literal(LiteralValue {
            value: serde_json::Value::String(date_str.to_string()),
            value_type: "date".to_string(),
        }));
    }

    // THH:MM:SS
    literal("T").parse_next(input)?;
    let _hour: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;
    literal(":").parse_next(input)?;
    let _min: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;
    literal(":").parse_next(input)?;
    let _sec: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;

    // Optional fractional seconds
    if input.starts_with('.') {
        *input = &input[1..];
        let _frac: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;
    }

    // Check for timezone
    if input.starts_with('+') || input.starts_with('-') {
        let _sign = &input[..1];
        *input = &input[1..];
        let _tz_h: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;
        literal(":").parse_next(input)?;
        let _tz_m: &str = take_while(2..=2, |c: char| c.is_ascii_digit()).parse_next(input)?;

        let consumed = start.len() - input.len();
        let dt_str = &start[..consumed];
        return Ok(Value::Literal(LiteralValue {
            value: serde_json::Value::String(dt_str.to_string()),
            value_type: "datetime-tz".to_string(),
        }));
    }

    let consumed = start.len() - input.len();
    let dt_str = &start[..consumed];
    Ok(Value::Literal(LiteralValue {
        value: serde_json::Value::String(dt_str.to_string()),
        value_type: "datetime".to_string(),
    }))
}

/// Parse an ISO 8601 duration literal: `P[nY][nM][nW][nD][T[nH][nM][nS]]`.
fn parse_duration_literal(input: &mut &str) -> PResult<Value> {
    let start = *input;
    literal("P").parse_next(input)?;

    // Consume duration components
    let mut has_component = false;
    loop {
        let _digits: &str = take_while(0.., |c: char| c.is_ascii_digit()).parse_next(input)?;
        if _digits.is_empty() {
            // Check for 'T' time separator
            if input.starts_with('T') {
                *input = &input[1..];
                has_component = true;
                continue;
            }
            break;
        }
        // Expect a designator letter after digits
        if let Some(c) = input.chars().next()
            && "YMWDHmSs".contains(c)
        {
            *input = &input[c.len_utf8()..];
            has_component = true;
            continue;
        }
        break;
    }

    if !has_component {
        return Err(ContextError::new());
    }

    let consumed = start.len() - input.len();
    let dur_str = &start[..consumed];
    Ok(Value::Literal(LiteralValue {
        value: serde_json::Value::String(dur_str.to_string()),
        value_type: "duration".to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Constraint parsers
// ---------------------------------------------------------------------------

/// Parse a constraint (inside entity/relation patterns).
fn parse_constraint(input: &mut &str) -> PResult<Constraint> {
    alt((
        parse_iid_constraint,
        parse_has_constraint,
        parse_isa_constraint,
    ))
    .parse_next(input)
}

/// Parse `iid 0x...`.
fn parse_iid_constraint(input: &mut &str) -> PResult<Constraint> {
    literal("iid").parse_next(input)?;
    ws_comments_required(input)?;
    let id = hex_id(input)?;
    Ok(Constraint::Iid(id.to_string()))
}

/// Parse `has attr_name value`.
fn parse_has_constraint(input: &mut &str) -> PResult<Constraint> {
    literal("has").parse_next(input)?;
    ws_comments_required(input)?;
    let attr_name = identifier(input)?;
    ws_comments_required(input)?;
    let value = parse_value(input)?;
    Ok(Constraint::Has {
        attr_name: attr_name.to_string(),
        value,
    })
}

/// Parse `isa type_name` or `isa! type_name`.
fn parse_isa_constraint(input: &mut &str) -> PResult<Constraint> {
    let strict = if input.starts_with("isa!") {
        literal("isa!").parse_next(input)?;
        true
    } else {
        literal("isa").parse_next(input)?;
        false
    };
    ws_comments_required(input)?;
    let type_name = identifier(input)?;
    Ok(Constraint::Isa {
        type_name: type_name.to_string(),
        strict,
    })
}

// ---------------------------------------------------------------------------
// Pattern parsers (match clause body)
// ---------------------------------------------------------------------------

/// Parse a list of patterns separated by `;` (the body of a match clause).
fn parse_patterns(input: &mut &str) -> PResult<Vec<Pattern>> {
    let mut patterns = Vec::new();
    loop {
        ws_comments(input);
        if input.is_empty()
            || input.starts_with("insert")
            || input.starts_with("put")
            || input.starts_with("delete")
            || input.starts_with("update")
            || input.starts_with("fetch")
            || input.starts_with("reduce")
            || input.starts_with("match")
            || input.starts_with("sort")
            || input.starts_with("limit")
            || input.starts_with("offset")
        {
            break;
        }
        let pattern = parse_pattern(input)?;
        patterns.push(pattern);
        ws_comments(input);
        // Consume optional semicolon
        let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    }
    Ok(patterns)
}

/// Parse a single pattern.
fn parse_pattern(input: &mut &str) -> PResult<Pattern> {
    ws_comments(input);
    alt((
        parse_not_pattern,
        parse_or_pattern,
        parse_variable_led_pattern,
    ))
    .parse_next(input)
}

/// Parse `not { patterns; }`.
fn parse_not_pattern(input: &mut &str) -> PResult<Pattern> {
    literal("not").parse_next(input)?;
    ws_comments(input);
    literal("{").parse_next(input)?;
    ws_comments(input);
    let inner = parse_inner_patterns(input)?;
    ws_comments(input);
    literal("}").parse_next(input)?;
    Ok(Pattern::Not(inner))
}

/// Parse `{ patterns; } or { patterns; } [or { patterns; }]*`.
fn parse_or_pattern(input: &mut &str) -> PResult<Pattern> {
    let first = parse_braced_patterns(input)?;
    ws_comments(input);
    literal("or").parse_next(input)?;
    ws_comments(input);
    let second = parse_braced_patterns(input)?;

    let mut alternatives = vec![first, second];
    loop {
        ws_comments(input);
        if opt(literal::<_, _, ContextError>("or"))
            .parse_next(input)?
            .is_none()
        {
            break;
        }
        ws_comments(input);
        let next = parse_braced_patterns(input)?;
        alternatives.push(next);
    }

    Ok(Pattern::Or(alternatives))
}

/// Parse `{ patterns; }` — patterns enclosed in braces.
fn parse_braced_patterns(input: &mut &str) -> PResult<Vec<Pattern>> {
    literal("{").parse_next(input)?;
    ws_comments(input);
    let patterns = parse_inner_patterns(input)?;
    ws_comments(input);
    literal("}").parse_next(input)?;
    Ok(patterns)
}

/// Parse patterns inside braces (used by Not and Or).
/// Patterns are separated by `;`.
fn parse_inner_patterns(input: &mut &str) -> PResult<Vec<Pattern>> {
    let mut patterns = Vec::new();
    loop {
        ws_comments(input);
        if input.starts_with('}') {
            break;
        }
        let pattern = parse_pattern(input)?;
        patterns.push(pattern);
        ws_comments(input);
        let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    }
    Ok(patterns)
}

/// Parse a pattern that starts with a variable `$var ...`.
fn parse_variable_led_pattern(input: &mut &str) -> PResult<Pattern> {
    let var = variable(input)?;
    ws_comments(input);

    // $var iid 0x...
    if input.starts_with("iid") {
        literal("iid").parse_next(input)?;
        ws_comments_required(input)?;
        let id = hex_id(input)?;
        return Ok(Pattern::Iid {
            variable: var,
            iid: id.to_string(),
        });
    }

    // $var sub parent_type
    if input.starts_with("sub") && !input.starts_with("sub!") {
        // Make sure "sub" is followed by whitespace, not part of another keyword
        let after_sub = &input[3..];
        if after_sub.starts_with(|c: char| c.is_ascii_whitespace()) {
            literal("sub").parse_next(input)?;
            ws_comments_required(input)?;
            let parent = identifier(input)?;
            return Ok(Pattern::SubType {
                variable: var,
                parent_type: parent.to_string(),
            });
        }
    }

    // $var has attr_type $attr_var
    if input.starts_with("has") {
        let saved = *input;
        literal("has").parse_next(input)?;
        ws_comments_required(input)?;
        let attr_type = identifier(input)?;
        ws_comments_required(input)?;
        // The attr_var must be a variable ($...)
        if input.starts_with('$') {
            let attr_var = variable(input)?;
            return Ok(Pattern::Has {
                thing_var: var,
                attr_type: attr_type.to_string(),
                attr_var,
            });
        }
        // Not a Has pattern (value is not a variable), restore
        *input = saved;
    }

    // $var isa[!] type_name ...
    if input.starts_with("isa") {
        let is_strict = if input.starts_with("isa!") {
            literal("isa!").parse_next(input)?;
            true
        } else {
            literal("isa").parse_next(input)?;
            false
        };
        ws_comments_required(input)?;
        let type_name = identifier(input)?;
        ws_comments(input);

        // Check for relation pattern: (role: $player, ...)
        if input.starts_with('(') {
            let role_players = parse_role_players(input)?;
            ws_comments(input);

            // Optional trailing constraints
            let mut constraints = Vec::new();
            while input.starts_with(',') {
                literal(",").parse_next(input)?;
                ws_comments(input);
                let c = parse_constraint(input)?;
                constraints.push(c);
                ws_comments(input);
            }

            return Ok(Pattern::Relation {
                variable: var,
                type_name: type_name.to_string(),
                role_players,
                constraints,
            });
        }

        // Check for entity constraints: , has ... | , iid ...
        if input.starts_with(',') {
            let mut constraints = Vec::new();
            while input.starts_with(',') {
                literal(",").parse_next(input)?;
                ws_comments(input);
                let c = parse_constraint(input)?;
                constraints.push(c);
                ws_comments(input);
            }
            return Ok(Pattern::Entity {
                variable: var,
                type_name: type_name.to_string(),
                constraints,
                is_strict,
            });
        }

        // Check for Attribute with value: $var isa type; $same_var value
        // The compiler outputs this as a single compiled pattern containing `;`
        let saved = *input;
        if input.starts_with(';') {
            // Peek: consume ; ws $same_var value
            let peek_input = &input[1..]; // skip ;
            let mut peek = peek_input;
            ws_comments(&mut peek);
            if peek.starts_with(&var as &str) {
                // Try to parse: $same_var value
                let mut try_input = peek;
                if let Ok(parsed_var) = variable(&mut try_input)
                    && parsed_var == var
                {
                    ws_comments(&mut try_input);
                    if let Ok(value) = parse_value(&mut try_input) {
                        // Commit: advance input past the consumed content
                        *input = try_input;
                        return Ok(Pattern::Attribute {
                            variable: var,
                            type_name: type_name.to_string(),
                            value: Some(value),
                        });
                    }
                }
            }
            *input = saved;
        }

        // Default: Entity with no constraints (same output as Attribute { value: None })
        return Ok(Pattern::Entity {
            variable: var,
            type_name: type_name.to_string(),
            constraints: vec![],
            is_strict,
        });
    }

    // $var op value (ValueComparison)
    let op = parse_comparison_op(input)?;
    ws_comments(input);
    let value = parse_value(input)?;
    Ok(Pattern::ValueComparison {
        var,
        operator: op.to_string(),
        value,
    })
}

/// Parse a comparison operator.
fn parse_comparison_op<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt((
        literal(">="),
        literal("<="),
        literal("!="),
        literal("=="),
        literal(">"),
        literal("<"),
        literal("contains"),
        literal("like"),
    ))
    .parse_next(input)
}

/// Parse `(role: $player, ...)` role player list.
fn parse_role_players(input: &mut &str) -> PResult<Vec<RolePlayer>> {
    literal("(").parse_next(input)?;
    ws_comments(input);
    let players: Vec<RolePlayer> = separated(
        1..,
        |i: &mut &str| {
            ws_comments(i);
            let role = identifier(i)?;
            ws_comments(i);
            literal(":").parse_next(i)?;
            ws_comments(i);
            let player_var = variable(i)?;
            ws_comments(i);
            Ok(RolePlayer {
                role: role.to_string(),
                player_var,
            })
        },
        ",",
    )
    .parse_next(input)?;
    ws_comments(input);
    literal(")").parse_next(input)?;
    Ok(players)
}

// ---------------------------------------------------------------------------
// Statement parsers (insert/delete/update clause body)
// ---------------------------------------------------------------------------

/// Context for statement parsing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StmtContext {
    Insert,
    Delete,
    Update,
}

/// Parse a list of statements separated by `;`.
fn parse_statements(input: &mut &str, ctx: StmtContext) -> PResult<Vec<Statement>> {
    let mut stmts = Vec::new();
    loop {
        ws_comments(input);
        if input.is_empty()
            || input.starts_with("match")
            || input.starts_with("insert")
            || input.starts_with("put")
            || input.starts_with("delete")
            || input.starts_with("update")
            || input.starts_with("fetch")
            || input.starts_with("reduce")
            || input.starts_with("sort")
            || input.starts_with("limit")
            || input.starts_with("offset")
        {
            break;
        }
        let stmt = parse_statement(input, ctx)?;
        stmts.push(stmt);
        ws_comments(input);
        let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    }
    Ok(stmts)
}

/// Parse a single statement.
fn parse_statement(input: &mut &str, ctx: StmtContext) -> PResult<Statement> {
    ws_comments(input);

    // Relation without variable: (role: $p, ...) isa type [, has ...]
    if input.starts_with('(') {
        return parse_relation_stmt_no_var(input);
    }

    // Must start with $var
    let var = variable(input)?;
    ws_comments(input);

    // $var isa type, links (role: $p, ...) — Relation with variable
    if input.starts_with("isa") {
        literal("isa").parse_next(input)?;
        ws_comments_required(input)?;
        let type_name = identifier(input)?;
        ws_comments(input);

        // Check for ", links" → Relation
        if input.starts_with(',') {
            let saved2 = *input;
            literal(",").parse_next(input)?;
            ws_comments(input);
            if input.starts_with("links") {
                literal("links").parse_next(input)?;
                ws_comments(input);
                let role_players = parse_role_players(input)?;
                ws_comments(input);

                // Optional inline attributes: , has attr value, ...
                let attrs = parse_inline_has_attrs(input)?;

                return Ok(Statement::Relation {
                    variable: var,
                    type_name: type_name.to_string(),
                    role_players,
                    include_variable: true,
                    attributes: attrs,
                });
            }
            // Not "links", restore to before comma
            *input = saved2;
        }

        // Plain Isa statement
        return Ok(Statement::Isa {
            variable: var,
            type_name: type_name.to_string(),
        });
    }

    // $var has attr_name value — Has statement
    if input.starts_with("has") {
        literal("has").parse_next(input)?;
        ws_comments_required(input)?;
        let attr_name = identifier(input)?;
        ws_comments_required(input)?;
        let value = parse_value(input)?;
        return Ok(Statement::Has {
            subject_var: var,
            attr_name: attr_name.to_string(),
            value,
        });
    }

    // Bare $var in delete context → DeleteThing
    if ctx == StmtContext::Delete {
        return Ok(Statement::DeleteThing(var));
    }

    Err(ContextError::new())
}

/// Parse `(role: $p, ...) isa type [, has attr value, ...]`.
fn parse_relation_stmt_no_var(input: &mut &str) -> PResult<Statement> {
    let role_players = parse_role_players(input)?;
    ws_comments(input);
    literal("isa").parse_next(input)?;
    ws_comments_required(input)?;
    let type_name = identifier(input)?;
    ws_comments(input);

    let attrs = parse_inline_has_attrs(input)?;

    Ok(Statement::Relation {
        variable: String::new(),
        type_name: type_name.to_string(),
        role_players,
        include_variable: false,
        attributes: attrs,
    })
}

/// Parse optional inline `has` attributes: `, has attr value [, has attr value ...]`.
fn parse_inline_has_attrs(input: &mut &str) -> PResult<Vec<Statement>> {
    let mut attrs = Vec::new();
    loop {
        ws_comments(input);
        if !input.starts_with(',') {
            break;
        }
        let saved = *input;
        literal(",").parse_next(input)?;
        ws_comments(input);
        if !input.starts_with("has") {
            *input = saved;
            break;
        }
        literal("has").parse_next(input)?;
        ws_comments_required(input)?;
        let attr_name = identifier(input)?;
        ws_comments_required(input)?;
        let value = parse_value(input)?;
        attrs.push(Statement::Has {
            subject_var: String::new(),
            attr_name: attr_name.to_string(),
            value,
        });
    }
    Ok(attrs)
}

// ---------------------------------------------------------------------------
// Fetch item parsers
// ---------------------------------------------------------------------------

/// Parse the body of a fetch clause: `{ items }`.
fn parse_fetch_items(input: &mut &str) -> PResult<Vec<FetchItem>> {
    literal("{").parse_next(input)?;
    ws_comments(input);

    let items: Vec<FetchItem> = separated(
        1..,
        |i: &mut &str| {
            ws_comments(i);
            parse_fetch_item(i)
        },
        ",",
    )
    .parse_next(input)?;

    ws_comments(input);
    // Optional trailing comma
    let _ = opt(literal::<_, _, ContextError>(",")).parse_next(input);
    ws_comments(input);
    literal("}").parse_next(input)?;
    Ok(items)
}

/// Parse a single fetch item: `"key": value`.
fn parse_fetch_item(input: &mut &str) -> PResult<FetchItem> {
    let key = quoted_string(input)?;
    ws_comments(input);
    literal(":").parse_next(input)?;
    ws_comments(input);

    alt((
        // { $var.* } → NestedWildcard
        |i: &mut &str| {
            literal("{").parse_next(i)?;
            ws_comments(i);
            let var = variable(i)?;
            literal(".*").parse_next(i)?;
            ws_comments(i);
            literal("}").parse_next(i)?;
            Ok(FetchItem::NestedWildcard {
                key: key.clone(),
                var,
            })
        },
        // [$var.attr] → AttributeList
        |i: &mut &str| {
            literal("[").parse_next(i)?;
            let var = variable(i)?;
            literal(".").parse_next(i)?;
            let attr_name = identifier(i)?;
            literal("]").parse_next(i)?;
            Ok(FetchItem::AttributeList {
                key: key.clone(),
                var,
                attr_name: attr_name.to_string(),
            })
        },
        // func($var) → Function (identifier followed by parenthesized variable)
        |i: &mut &str| {
            let func_name = identifier(i)?;
            literal("(").parse_next(i)?;
            let var = variable(i)?;
            literal(")").parse_next(i)?;
            Ok(FetchItem::Function {
                key: key.clone(),
                func_name: func_name.to_string(),
                var,
            })
        },
        // $var.* → Wildcard
        |i: &mut &str| {
            let var = variable(i)?;
            literal(".*").parse_next(i)?;
            Ok(FetchItem::Wildcard {
                key: key.clone(),
                var,
            })
        },
        // $var.attr → Attribute
        |i: &mut &str| {
            let var = variable(i)?;
            literal(".").parse_next(i)?;
            let attr_name = identifier(i)?;
            Ok(FetchItem::Attribute {
                key: key.clone(),
                var,
                attr_name: attr_name.to_string(),
            })
        },
        // $var → Variable
        |i: &mut &str| {
            let var = variable(i)?;
            Ok(FetchItem::Variable {
                key: key.clone(),
                var,
            })
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Let assignment parser
// ---------------------------------------------------------------------------

/// Parse `let $var1 [, $var2, ...] (= | in) expression`.
fn parse_let_assignment(input: &mut &str) -> PResult<LetAssignment> {
    literal("let").parse_next(input)?;
    ws_comments_required(input)?;

    let vars: Vec<String> = separated(
        1..,
        |i: &mut &str| {
            ws_comments(i);
            variable(i)
        },
        ",",
    )
    .parse_next(input)?;

    ws_comments(input);

    let is_stream =
        if input.starts_with("in") && input[2..].starts_with(|c: char| c.is_ascii_whitespace()) {
            literal("in").parse_next(input)?;
            true
        } else {
            literal("=").parse_next(input)?;
            false
        };

    ws_comments(input);
    let expression = parse_value(input)?;

    Ok(LetAssignment {
        variables: vars,
        expression,
        is_stream,
    })
}

// ---------------------------------------------------------------------------
// Clause parsers
// ---------------------------------------------------------------------------

/// Parse a sequence of clauses.
fn parse_clauses(input: &mut &str) -> PResult<Vec<Clause>> {
    let mut clauses = Vec::new();
    loop {
        ws_comments(input);
        if input.is_empty() {
            break;
        }
        let clause = parse_clause(input)?;
        clauses.push(clause);
    }
    if clauses.is_empty() {
        return Err(ContextError::new());
    }
    Ok(clauses)
}

/// Parse a single clause.
fn parse_clause(input: &mut &str) -> PResult<Clause> {
    ws_comments(input);
    alt((
        parse_match_clause,
        parse_insert_clause,
        parse_put_clause,
        parse_delete_clause,
        parse_update_clause,
        parse_fetch_clause,
        parse_reduce_clause,
        parse_sort_clause,
        parse_limit_clause,
        parse_offset_clause,
    ))
    .parse_next(input)
}

/// Parse a match clause: `match\n<patterns|let-assignments>;`.
fn parse_match_clause(input: &mut &str) -> PResult<Clause> {
    literal("match").parse_next(input)?;
    ws_comments(input);

    // Peek: does the body start with "let"?
    if input.starts_with("let") && input[3..].starts_with(|c: char| c.is_ascii_whitespace()) {
        // Try MatchLet
        let saved = *input;
        if let Ok(assignments) = parse_let_assignments(input) {
            return Ok(Clause::MatchLet(assignments));
        }
        *input = saved;
    }

    let patterns = parse_patterns(input)?;
    Ok(Clause::Match(patterns))
}

/// Parse a list of let assignments separated by `;`.
fn parse_let_assignments(input: &mut &str) -> PResult<Vec<LetAssignment>> {
    let mut assignments = Vec::new();
    loop {
        ws_comments(input);
        if !input.starts_with("let") {
            break;
        }
        let assign = parse_let_assignment(input)?;
        assignments.push(assign);
        ws_comments(input);
        let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    }
    if assignments.is_empty() {
        return Err(ContextError::new());
    }
    Ok(assignments)
}

/// Parse an insert clause: `insert\n<statements>;`.
fn parse_insert_clause(input: &mut &str) -> PResult<Clause> {
    literal("insert").parse_next(input)?;
    ws_comments(input);
    let stmts = parse_statements(input, StmtContext::Insert)?;
    Ok(Clause::Insert(stmts))
}

/// Parse a put clause: `put\n<statements>;`.
fn parse_put_clause(input: &mut &str) -> PResult<Clause> {
    literal("put").parse_next(input)?;
    ws_comments(input);
    let stmts = parse_statements(input, StmtContext::Insert)?;
    Ok(Clause::Put(stmts))
}

/// Parse a delete clause: `delete\n<statements>;`.
fn parse_delete_clause(input: &mut &str) -> PResult<Clause> {
    literal("delete").parse_next(input)?;
    ws_comments(input);
    let stmts = parse_statements(input, StmtContext::Delete)?;
    Ok(Clause::Delete(stmts))
}

/// Parse an update clause: `update\n<statements>;`.
fn parse_update_clause(input: &mut &str) -> PResult<Clause> {
    literal("update").parse_next(input)?;
    ws_comments(input);
    let stmts = parse_statements(input, StmtContext::Update)?;
    Ok(Clause::Update(stmts))
}

/// Parse a fetch clause: `fetch { items };`.
fn parse_fetch_clause(input: &mut &str) -> PResult<Clause> {
    literal("fetch").parse_next(input)?;
    ws_comments(input);
    let items = parse_fetch_items(input)?;
    ws_comments(input);
    let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    Ok(Clause::Fetch(items))
}

/// Parse a reduce clause: `reduce $var1 = expr1 [, ...] [groupby $var];`.
fn parse_reduce_clause(input: &mut &str) -> PResult<Clause> {
    literal("reduce").parse_next(input)?;
    ws_comments_required(input)?;

    let assignments: Vec<ReduceAssignment> = separated(
        1..,
        |i: &mut &str| {
            ws_comments(i);
            let var = variable(i)?;
            ws_comments(i);
            literal("=").parse_next(i)?;
            ws_comments(i);
            let expr = parse_value(i)?;
            ws_comments(i);
            Ok(ReduceAssignment {
                variable: var,
                expression: expr,
            })
        },
        ",",
    )
    .parse_next(input)?;

    ws_comments(input);

    let group_by = if input.starts_with("groupby") {
        literal("groupby").parse_next(input)?;
        ws_comments_required(input)?;
        let var = variable(input)?;
        Some(var)
    } else {
        None
    };

    ws_comments(input);
    let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);

    Ok(Clause::Reduce {
        assignments,
        group_by,
    })
}

/// Parse a sort clause: `sort $var asc[, $var2 desc, ...];`.
fn parse_sort_clause(input: &mut &str) -> PResult<Clause> {
    literal("sort").parse_next(input)?;
    ws_comments_required(input)?;

    let fields: Vec<SortField> = separated(
        1..,
        |i: &mut &str| {
            ws_comments(i);
            let var = variable(i)?;
            ws_comments_required(i)?;
            let dir = alt((literal("asc"), literal("desc"))).parse_next(i)?;
            ws_comments(i);
            Ok(SortField {
                variable: var,
                ascending: dir == "asc",
            })
        },
        ",",
    )
    .parse_next(input)?;

    ws_comments(input);
    let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);

    Ok(Clause::Sort(fields))
}

/// Parse a limit clause: `limit N;`.
fn parse_limit_clause(input: &mut &str) -> PResult<Clause> {
    literal("limit").parse_next(input)?;
    ws_comments_required(input)?;
    let digits: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;
    let n: u64 = digits.parse().map_err(|_| ContextError::new())?;
    ws_comments(input);
    let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    Ok(Clause::Limit(n))
}

/// Parse an offset clause: `offset N;`.
fn parse_offset_clause(input: &mut &str) -> PResult<Clause> {
    literal("offset").parse_next(input)?;
    ws_comments_required(input)?;
    let digits: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;
    let n: u64 = digits.parse().map_err(|_| ContextError::new())?;
    ws_comments(input);
    let _ = opt(literal::<_, _, ContextError>(";")).parse_next(input);
    Ok(Clause::Offset(n))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::QueryCompiler;
    use serde_json::json;

    // === Token tests ===

    #[test]
    fn test_parse_variable() {
        let mut input = "$person";
        assert_eq!(variable(&mut input).unwrap(), "$person");
        assert!(input.is_empty());
    }

    #[test]
    fn test_parse_variable_with_hyphen() {
        let mut input = "$my-var";
        assert_eq!(variable(&mut input).unwrap(), "$my-var");
    }

    #[test]
    fn test_parse_quoted_string() {
        let mut input = r#""hello world""#;
        assert_eq!(quoted_string(&mut input).unwrap(), "hello world");
    }

    #[test]
    fn test_parse_quoted_string_with_escapes() {
        let mut input = r#""line1\nline2\ttab\\slash\"quote""#;
        assert_eq!(
            quoted_string(&mut input).unwrap(),
            "line1\nline2\ttab\\slash\"quote"
        );
    }

    #[test]
    fn test_parse_hex_id() {
        let mut input = "0x0000000000000001";
        assert_eq!(hex_id(&mut input).unwrap(), "0x0000000000000001");
    }

    // === Value tests ===

    #[test]
    fn test_parse_string_value() {
        let mut input = r#""Alice""#;
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("Alice"));
                assert_eq!(lit.value_type, "string");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_boolean_value() {
        let mut input = "true";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(true));
                assert_eq!(lit.value_type, "boolean");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_long_value() {
        let mut input = "42";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(42));
                assert_eq!(lit.value_type, "long");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_negative_long() {
        let mut input = "-100";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(-100));
                assert_eq!(lit.value_type, "long");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_double_value() {
        let mut input = "3.15";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value_type, "double");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_decimal_value() {
        let mut input = "42dec";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(42));
                assert_eq!(lit.value_type, "decimal");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_decimal_with_fraction() {
        let mut input = "123.45dec";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value_type, "decimal");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_date_value() {
        let mut input = "2024-01-15";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15"));
                assert_eq!(lit.value_type, "date");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_datetime_value() {
        let mut input = "2024-01-15T10:30:00";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15T10:30:00"));
                assert_eq!(lit.value_type, "datetime");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_datetime_tz_value() {
        let mut input = "2024-01-15T10:30:00+05:30";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15T10:30:00+05:30"));
                assert_eq!(lit.value_type, "datetime-tz");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_duration_value() {
        let mut input = "P1Y2M3D";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("P1Y2M3D"));
                assert_eq!(lit.value_type, "duration");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_duration_with_time() {
        let mut input = "PT5H30M";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("PT5H30M"));
                assert_eq!(lit.value_type, "duration");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_variable_value() {
        let mut input = "$person";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Variable(v) => assert_eq!(v, "$person"),
            _ => panic!("expected Variable"),
        }
    }

    #[test]
    fn test_parse_function_call() {
        let mut input = "count($p)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "count");
                assert_eq!(f.args.len(), 1);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_function_call_multiple_args() {
        let mut input = "max($x, $y)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "max");
                assert_eq!(f.args.len(), 2);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_function_call_no_args() {
        let mut input = "my_func()";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "my_func");
                assert!(f.args.is_empty());
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_arithmetic_value() {
        let mut input = "($x + 5)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Arithmetic(a) => {
                assert_eq!(a.operator, "+");
            }
            _ => panic!("expected Arithmetic"),
        }
    }

    // === Constraint tests ===

    #[test]
    fn test_parse_iid_constraint() {
        let mut input = "iid 0x123abc";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Iid(id) => assert_eq!(id, "0x123abc"),
            _ => panic!("expected Iid"),
        }
    }

    #[test]
    fn test_parse_has_constraint() {
        let mut input = r#"has name "Alice""#;
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Has { attr_name, value } => {
                assert_eq!(attr_name, "name");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Alice")),
                    _ => panic!("expected Literal value"),
                }
            }
            _ => panic!("expected Has"),
        }
    }

    #[test]
    fn test_parse_isa_constraint() {
        let mut input = "isa person";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Isa { type_name, strict } => {
                assert_eq!(type_name, "person");
                assert!(!strict);
            }
            _ => panic!("expected Isa"),
        }
    }

    #[test]
    fn test_parse_isa_strict_constraint() {
        let mut input = "isa! person";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Isa { type_name, strict } => {
                assert_eq!(type_name, "person");
                assert!(strict);
            }
            _ => panic!("expected Isa"),
        }
    }

    // === Pattern tests ===

    #[test]
    fn test_parse_entity_pattern() {
        let mut input = r#"$p isa person, has name "Alice""#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity {
                variable,
                type_name,
                constraints,
                is_strict,
            } => {
                assert_eq!(variable, "$p");
                assert_eq!(type_name, "person");
                assert!(!is_strict);
                assert_eq!(constraints.len(), 1);
            }
            _ => panic!("expected Entity, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_entity_pattern_strict() {
        let mut input = "$p isa! person";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity { is_strict, .. } => assert!(is_strict),
            _ => panic!("expected Entity"),
        }
    }

    #[test]
    fn test_parse_entity_multiple_constraints() {
        let mut input = r#"$p isa person, iid 0x123, has name "Alice", has age 30"#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity { constraints, .. } => {
                assert_eq!(constraints.len(), 3);
            }
            _ => panic!("expected Entity"),
        }
    }

    #[test]
    fn test_parse_relation_pattern() {
        let mut input = "$r isa friendship (friend: $p, friend: $q)";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Relation {
                variable,
                type_name,
                role_players,
                constraints,
            } => {
                assert_eq!(variable, "$r");
                assert_eq!(type_name, "friendship");
                assert_eq!(role_players.len(), 2);
                assert_eq!(role_players[0].role, "friend");
                assert_eq!(role_players[0].player_var, "$p");
                assert!(constraints.is_empty());
            }
            _ => panic!("expected Relation, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_relation_with_constraints() {
        let mut input =
            r#"$r isa employment (employee: $p, employer: $c), has start-date 2024-01-15"#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Relation {
                role_players,
                constraints,
                ..
            } => {
                assert_eq!(role_players.len(), 2);
                assert_eq!(constraints.len(), 1);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_subtype_pattern() {
        let mut input = "$t sub entity";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::SubType {
                variable,
                parent_type,
            } => {
                assert_eq!(variable, "$t");
                assert_eq!(parent_type, "entity");
            }
            _ => panic!("expected SubType"),
        }
    }

    #[test]
    fn test_parse_has_pattern() {
        let mut input = "$p has name $n";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Has {
                thing_var,
                attr_type,
                attr_var,
            } => {
                assert_eq!(thing_var, "$p");
                assert_eq!(attr_type, "name");
                assert_eq!(attr_var, "$n");
            }
            _ => panic!("expected Has, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison() {
        let mut input = "$age > 18";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison {
                var,
                operator,
                value,
            } => {
                assert_eq!(var, "$age");
                assert_eq!(operator, ">");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!(18)),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected ValueComparison, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison_operators() {
        for (op_str, expected) in &[
            (">=", ">="),
            ("<=", "<="),
            ("!=", "!="),
            ("==", "=="),
            (">", ">"),
            ("<", "<"),
        ] {
            let input_str = format!("$x {} 10", op_str);
            let mut input = input_str.as_str();
            let p = parse_pattern(&mut input).unwrap();
            match p {
                Pattern::ValueComparison { operator, .. } => {
                    assert_eq!(operator, *expected);
                }
                _ => panic!("expected ValueComparison for {}", op_str),
            }
        }
    }

    #[test]
    fn test_parse_not_pattern() {
        let mut input = "not { $p isa person; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Not(inner) => {
                assert_eq!(inner.len(), 1);
            }
            _ => panic!("expected Not"),
        }
    }

    #[test]
    fn test_parse_or_pattern() {
        let mut input = "{ $p isa person; } or { $e isa employee; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Or(alternatives) => {
                assert_eq!(alternatives.len(), 2);
                assert_eq!(alternatives[0].len(), 1);
                assert_eq!(alternatives[1].len(), 1);
            }
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_parse_or_pattern_three_alternatives() {
        let mut input = "{ $a isa a; } or { $b isa b; } or { $c isa c; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Or(alternatives) => assert_eq!(alternatives.len(), 3),
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_parse_iid_pattern() {
        let mut input = "$p iid 0x0000000000000001";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Iid { variable, iid } => {
                assert_eq!(variable, "$p");
                assert_eq!(iid, "0x0000000000000001");
            }
            _ => panic!("expected Iid"),
        }
    }

    #[test]
    fn test_parse_attribute_pattern_with_value() {
        let mut input = r#"$a isa name; $a "John""#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Attribute {
                variable,
                type_name,
                value,
            } => {
                assert_eq!(variable, "$a");
                assert_eq!(type_name, "name");
                assert!(value.is_some());
                match value.unwrap() {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("John")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected Attribute, got {:?}", p),
        }
    }

    // === Statement tests ===

    #[test]
    fn test_parse_has_statement() {
        let mut input = r#"$p has name "Alice""#;
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Has {
                subject_var,
                attr_name,
                value,
            } => {
                assert_eq!(subject_var, "$p");
                assert_eq!(attr_name, "name");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Alice")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected Has"),
        }
    }

    #[test]
    fn test_parse_isa_statement() {
        let mut input = "$p isa person";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Isa {
                variable,
                type_name,
            } => {
                assert_eq!(variable, "$p");
                assert_eq!(type_name, "person");
            }
            _ => panic!("expected Isa"),
        }
    }

    #[test]
    fn test_parse_relation_stmt_with_var() {
        let mut input = "$r isa friendship, links (friend: $p, friend: $q)";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation {
                variable,
                type_name,
                role_players,
                include_variable,
                attributes,
            } => {
                assert_eq!(variable, "$r");
                assert_eq!(type_name, "friendship");
                assert!(include_variable);
                assert_eq!(role_players.len(), 2);
                assert!(attributes.is_empty());
            }
            _ => panic!("expected Relation, got {:?}", s),
        }
    }

    #[test]
    fn test_parse_relation_stmt_no_var() {
        let mut input = "(employee: $p, employer: $c) isa employment";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation {
                include_variable,
                role_players,
                ..
            } => {
                assert!(!include_variable);
                assert_eq!(role_players.len(), 2);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_relation_stmt_with_attrs() {
        let mut input = "(employee: $p, employer: $c) isa employment, has start-date 2024-01-15";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation { attributes, .. } => {
                assert_eq!(attributes.len(), 1);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_delete_thing() {
        let mut input = "$p";
        let s = parse_statement(&mut input, StmtContext::Delete).unwrap();
        match s {
            Statement::DeleteThing(var) => assert_eq!(var, "$p"),
            _ => panic!("expected DeleteThing"),
        }
    }

    // === Fetch item tests ===

    #[test]
    fn test_parse_fetch_attribute() {
        let mut input = r#""name": $p.name"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Attribute {
                key,
                var,
                attr_name,
            } => {
                assert_eq!(key, "name");
                assert_eq!(var, "$p");
                assert_eq!(attr_name, "name");
            }
            _ => panic!("expected Attribute, got {:?}", item),
        }
    }

    #[test]
    fn test_parse_fetch_variable() {
        let mut input = r#""person": $p"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Variable { key, var } => {
                assert_eq!(key, "person");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Variable"),
        }
    }

    #[test]
    fn test_parse_fetch_attribute_list() {
        let mut input = r#""names": [$p.name]"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::AttributeList {
                key,
                var,
                attr_name,
            } => {
                assert_eq!(key, "names");
                assert_eq!(var, "$p");
                assert_eq!(attr_name, "name");
            }
            _ => panic!("expected AttributeList"),
        }
    }

    #[test]
    fn test_parse_fetch_function() {
        let mut input = r#""id": iid($p)"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Function {
                key,
                func_name,
                var,
            } => {
                assert_eq!(key, "id");
                assert_eq!(func_name, "iid");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Function"),
        }
    }

    #[test]
    fn test_parse_fetch_wildcard() {
        let mut input = r#""all": $p.*"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Wildcard { key, var } => {
                assert_eq!(key, "all");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Wildcard"),
        }
    }

    #[test]
    fn test_parse_fetch_nested_wildcard() {
        let mut input = r#""nested": { $p.* }"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::NestedWildcard { key, var } => {
                assert_eq!(key, "nested");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected NestedWildcard"),
        }
    }

    // === Clause tests ===

    #[test]
    fn test_parse_match_clause() {
        let input = "match\n$p isa person;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Match(patterns) => {
                assert_eq!(patterns.len(), 1);
            }
            _ => panic!("expected Match"),
        }
    }

    #[test]
    fn test_parse_insert_clause() {
        let input = "insert\n$p isa person;\n$p has name \"Alice\";";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Insert(stmts) => {
                assert_eq!(stmts.len(), 2);
            }
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn test_parse_put_clause() {
        let input = "put\n$p isa person;\n$p has name \"Alice\";";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Put(stmts) => {
                assert_eq!(stmts.len(), 2);
            }
            _ => panic!("expected Put"),
        }
    }

    #[test]
    fn test_parse_match_put() {
        let input = "match\n$p isa person;\nput\n$p has age 30;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Put(_)));
    }

    #[test]
    fn test_parse_delete_clause() {
        let input = "delete\n$p;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Delete(stmts) => {
                assert_eq!(stmts.len(), 1);
                match &stmts[0] {
                    Statement::DeleteThing(v) => assert_eq!(v, "$p"),
                    _ => panic!("expected DeleteThing"),
                }
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_parse_update_clause() {
        let input = "update\n$p has age 31;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Update(stmts) => assert_eq!(stmts.len(), 1),
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_fetch_clause() {
        let input = "fetch {\n  \"name\": $p.name,\n  \"age\": [$p.age]\n};";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Fetch(items) => assert_eq!(items.len(), 2),
            _ => panic!("expected Fetch"),
        }
    }

    #[test]
    fn test_parse_match_let_clause() {
        let input = "match\nlet $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].variables, vec!["$count"]);
                assert!(!assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet, got {:?}", clauses[0]),
        }
    }

    #[test]
    fn test_parse_match_let_stream() {
        let input = "match\nlet $item in get_items();";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert!(assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet"),
        }
    }

    #[test]
    fn test_parse_match_let_multi_vars() {
        let input = "match\nlet $type, $count in count_by_type();";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert_eq!(assignments[0].variables, vec!["$type", "$count"]);
                assert!(assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet"),
        }
    }

    #[test]
    fn test_parse_reduce_clause() {
        let input = "reduce $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Reduce {
                assignments,
                group_by,
            } => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].variable, "$count");
                assert!(group_by.is_none());
            }
            _ => panic!("expected Reduce"),
        }
    }

    #[test]
    fn test_parse_reduce_with_groupby() {
        let input = "reduce $count = count($p) groupby $dept;";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Reduce {
                assignments,
                group_by,
            } => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(group_by, &Some("$dept".to_string()));
            }
            _ => panic!("expected Reduce"),
        }
    }

    #[test]
    fn test_parse_reduce_multiple_assignments() {
        let input = "reduce $count = count($p), $total = sum($salary);";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Reduce { assignments, .. } => {
                assert_eq!(assignments.len(), 2);
            }
            _ => panic!("expected Reduce"),
        }
    }

    // === Multi-clause tests ===

    #[test]
    fn test_parse_match_insert() {
        let input = "match\n$p isa person;\ninsert\n$p has age 30;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Insert(_)));
    }

    #[test]
    fn test_parse_match_delete() {
        let input = "match\n$p isa person;\ndelete\n$p;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Delete(_)));
    }

    #[test]
    fn test_parse_match_fetch() {
        let input = "match\n$p isa person;\nfetch {\n  \"name\": $p.name\n};";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Fetch(_)));
    }

    #[test]
    fn test_parse_match_reduce() {
        let input = "match\n$p isa person;\nreduce $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Reduce { .. }));
    }

    // === Roundtrip tests ===

    fn assert_roundtrip(typeql: &str) {
        let compiler = QueryCompiler::new();
        let parsed = parse_typeql_query(typeql).unwrap();
        let recompiled = compiler.compile(&parsed);
        assert_eq!(
            recompiled, typeql,
            "\n--- INPUT ---\n{}\n--- RECOMPILED ---\n{}",
            typeql, recompiled
        );
    }

    #[test]
    fn test_roundtrip_simple_match() {
        assert_roundtrip("match\n$p isa person;");
    }

    #[test]
    fn test_roundtrip_match_with_constraints() {
        assert_roundtrip("match\n$p isa person, has name \"Alice\", has age 30;");
    }

    #[test]
    fn test_roundtrip_match_strict() {
        assert_roundtrip("match\n$p isa! person;");
    }

    #[test]
    fn test_roundtrip_match_iid() {
        assert_roundtrip("match\n$p iid 0x0000000000000001;");
    }

    #[test]
    fn test_roundtrip_match_subtype() {
        assert_roundtrip("match\n$t sub entity;");
    }

    #[test]
    fn test_roundtrip_match_has_pattern() {
        assert_roundtrip("match\n$p has name $n;");
    }

    #[test]
    fn test_roundtrip_match_value_comparison() {
        assert_roundtrip("match\n$age > 18;");
    }

    #[test]
    fn test_roundtrip_match_not() {
        assert_roundtrip("match\nnot { $p isa person; };");
    }

    #[test]
    fn test_roundtrip_match_or() {
        assert_roundtrip("match\n{ $p isa person; } or { $e isa employee; };");
    }

    #[test]
    fn test_roundtrip_relation_match() {
        assert_roundtrip("match\n$r isa friendship (friend: $p, friend: $q);");
    }

    #[test]
    fn test_roundtrip_attribute_with_value() {
        assert_roundtrip("match\n$a isa name; $a \"John\";");
    }

    #[test]
    fn test_roundtrip_insert() {
        assert_roundtrip("insert\n$p isa person;\n$p has name \"Alice\";");
    }

    #[test]
    fn test_roundtrip_relation_insert_with_var() {
        assert_roundtrip(
            "match\n$p isa person;\ninsert\n$r isa friendship, links (friend: $p, friend: $q);",
        );
    }

    #[test]
    fn test_roundtrip_relation_insert_no_var() {
        assert_roundtrip("insert\n(employee: $p, employer: $c) isa employment;");
    }

    #[test]
    fn test_roundtrip_relation_insert_with_attrs() {
        assert_roundtrip(
            "insert\n(employee: $p, employer: $c) isa employment, has start-date 2024-01-15;",
        );
    }

    #[test]
    fn test_roundtrip_delete() {
        assert_roundtrip("match\n$p isa person;\ndelete\n$p;");
    }

    #[test]
    fn test_roundtrip_update() {
        assert_roundtrip("update\n$p has age 31;");
    }

    #[test]
    fn test_roundtrip_fetch() {
        assert_roundtrip("fetch {\n  \"name\": $p.name,\n  \"age\": [$p.age]\n};");
    }

    #[test]
    fn test_roundtrip_fetch_all_types() {
        assert_roundtrip(
            "fetch {\n  \"a\": $p.name,\n  \"b\": $p,\n  \"c\": [$p.nick],\n  \"d\": iid($p),\n  \"e\": $p.*,\n  \"f\": { $p.* }\n};",
        );
    }

    #[test]
    fn test_roundtrip_match_let() {
        assert_roundtrip("match\nlet $count = count($p);");
    }

    #[test]
    fn test_roundtrip_match_let_stream() {
        assert_roundtrip("match\nlet $item in get_items();");
    }

    #[test]
    fn test_roundtrip_match_let_multi_vars() {
        assert_roundtrip("match\nlet $type, $count in count_by_type();");
    }

    #[test]
    fn test_roundtrip_reduce() {
        assert_roundtrip("reduce $count = count($p);");
    }

    #[test]
    fn test_roundtrip_reduce_groupby() {
        assert_roundtrip("reduce $count = count($p) groupby $dept;");
    }

    #[test]
    fn test_roundtrip_reduce_multiple() {
        assert_roundtrip("reduce $count = count($p), $total = sum($salary);");
    }

    #[test]
    fn test_roundtrip_complex_match() {
        assert_roundtrip(
            "match\n$p isa person, has name \"Alice\", has age 30;\n$r isa friendship (friend: $p, friend: $q);\n$q isa person;\nnot { $q isa! employee; };",
        );
    }

    #[test]
    fn test_roundtrip_boolean_values() {
        assert_roundtrip("match\n$p isa person, has active true;");
    }

    #[test]
    fn test_roundtrip_decimal_value() {
        assert_roundtrip("match\n$p isa person, has salary 50000dec;");
    }

    #[test]
    fn test_roundtrip_date_value() {
        assert_roundtrip("match\n$p isa person, has birth-date 2024-01-15;");
    }

    #[test]
    fn test_roundtrip_datetime_value() {
        assert_roundtrip("match\n$p isa person, has created-at 2024-01-15T10:30:00;");
    }

    #[test]
    fn test_roundtrip_arithmetic() {
        assert_roundtrip("match\n$x > ($base + 100);");
    }

    #[test]
    fn test_roundtrip_function_call() {
        assert_roundtrip("match\nlet $count = count($p);");
    }

    // === Error tests ===

    #[test]
    fn test_error_empty_input() {
        let result = parse_typeql_query("").unwrap();
        assert!(result.is_empty());
        let result = parse_typeql_query("   \n  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_error_unrecognized_keyword() {
        assert!(parse_typeql_query("foobar $x;").is_err());
    }

    #[test]
    fn test_error_trailing_content() {
        assert!(parse_typeql_query("match\n$p isa person;\ngarbage").is_err());
    }

    // === Flexible whitespace tests ===

    #[test]
    fn test_flexible_whitespace() {
        // Extra spaces and different formatting should still parse
        let input = "match   \n  $p   isa   person  ,  has name   \"Alice\"  ;  ";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Match(_)));
    }

    #[test]
    fn test_comments_in_query() {
        let input = "match\n# find all persons\n$p isa person;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
    }

    #[test]
    fn test_multi_pattern_match() {
        let input = "match\n$p isa person;\n$p has name $n;\n$n > \"A\";";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Match(patterns) => assert_eq!(patterns.len(), 3),
            _ => panic!("expected Match"),
        }
    }

    // === Sort/Limit/Offset tests ===

    #[test]
    fn test_parse_sort_clause_single() {
        let input = "sort $age asc;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Sort(fields) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].variable, "$age");
                assert!(fields[0].ascending);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn test_parse_sort_clause_multiple() {
        let input = "sort $name asc, $age desc;";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Sort(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].variable, "$name");
                assert!(fields[0].ascending);
                assert_eq!(fields[1].variable, "$age");
                assert!(!fields[1].ascending);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn test_parse_limit_clause() {
        let input = "limit 10;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Limit(10)));
    }

    #[test]
    fn test_parse_offset_clause() {
        let input = "offset 20;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Offset(20)));
    }

    #[test]
    fn test_parse_match_sort_limit_offset() {
        let input = "match\n$p isa person;\nsort $age asc;\nlimit 10;\noffset 5;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 4);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Sort(_)));
        assert!(matches!(&clauses[2], Clause::Limit(10)));
        assert!(matches!(&clauses[3], Clause::Offset(5)));
    }

    // === Contains/Like operator tests ===

    #[test]
    fn test_parse_contains_operator() {
        let mut input = "contains";
        let op = parse_comparison_op(&mut input).unwrap();
        assert_eq!(op, "contains");
    }

    #[test]
    fn test_parse_like_operator() {
        let mut input = "like";
        let op = parse_comparison_op(&mut input).unwrap();
        assert_eq!(op, "like");
    }

    #[test]
    fn test_parse_value_comparison_contains() {
        let mut input = "$name contains \"Ali\"";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison {
                var,
                operator,
                value,
            } => {
                assert_eq!(var, "$name");
                assert_eq!(operator, "contains");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Ali")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected ValueComparison, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison_like() {
        let mut input = "$name like \"^A.*\"";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison { var, operator, .. } => {
                assert_eq!(var, "$name");
                assert_eq!(operator, "like");
            }
            _ => panic!("expected ValueComparison"),
        }
    }

    // === Sort/Limit/Offset roundtrip tests ===

    #[test]
    fn test_roundtrip_sort_single() {
        assert_roundtrip("match\n$p isa person;\nsort $age asc;");
    }

    #[test]
    fn test_roundtrip_sort_multiple() {
        assert_roundtrip("match\n$p isa person;\nsort $name asc, $age desc;");
    }

    #[test]
    fn test_roundtrip_limit() {
        assert_roundtrip("match\n$p isa person;\nlimit 10;");
    }

    #[test]
    fn test_roundtrip_offset() {
        assert_roundtrip("match\n$p isa person;\noffset 20;");
    }

    #[test]
    fn test_roundtrip_sort_limit_offset() {
        assert_roundtrip("match\n$p isa person;\nsort $age asc;\nlimit 10;\noffset 5;");
    }

    #[test]
    fn test_roundtrip_contains() {
        assert_roundtrip("match\n$name contains \"Ali\";");
    }

    #[test]
    fn test_roundtrip_like() {
        assert_roundtrip("match\n$name like \"^A.*\";");
    }
}
