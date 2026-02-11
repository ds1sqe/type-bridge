//! Winnow-based parser for TypeQL `define` blocks.
//!
//! Converts TypeQL schema text into [`TypeSchema`] structs.
//! Reference grammar: `type_bridge/generator/typeql.lark`

use winnow::combinator::{alt, delimited, opt, repeat, separated, terminated};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{literal, take_until, take_while};

use super::schema::{
    AttributeType, Cardinality, EntityType, OwnedAttribute, PlayedRole, RelationType, RoleSpec,
    SchemaError, TypeSchema,
};

/// Type alias for parser results.
type PResult<T> = winnow::error::Result<T>;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a TypeQL schema string into an unresolved [`TypeSchema`].
///
/// Handles one or more `define` blocks. Functions and structs are skipped.
pub fn parse_typeql(input: &str) -> Result<TypeSchema, SchemaError> {
    let mut input_ref = input;
    match parse_schema(&mut input_ref) {
        Ok(schema) => Ok(schema),
        Err(_e) => {
            let consumed = input.len() - input_ref.len();
            let parsed_portion = &input[..consumed];
            let line = parsed_portion.chars().filter(|c| *c == '\n').count() + 1;
            let last_newline = parsed_portion.rfind('\n').map(|i| i + 1).unwrap_or(0);
            let column = consumed - last_newline + 1;
            Err(SchemaError::ParseError {
                message: format!("unexpected input at line {}:{}", line, column),
                line,
                column,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level parsers
// ---------------------------------------------------------------------------

fn parse_schema(input: &mut &str) -> PResult<TypeSchema> {
    ws_comments(input);
    let blocks: Vec<Vec<Statement>> = repeat(0.., parse_define_block).parse_next(input)?;
    ws_comments(input);

    if !input.is_empty() {
        return Err(ContextError::new());
    }

    let mut schema = TypeSchema::new();
    for block in blocks {
        for stmt in block {
            match stmt {
                Statement::Attribute(attr) => {
                    schema.attributes.insert(attr.name.clone(), attr);
                }
                Statement::Entity(entity) => {
                    let name = entity.name.clone();
                    if let Some(existing) = schema.entities.get_mut(&name) {
                        // Merge: TypeQL allows split definitions for the same type
                        if entity.parent.is_some() {
                            existing.parent = entity.parent;
                        }
                        if entity.is_abstract {
                            existing.is_abstract = true;
                        }
                        for own in entity.owns {
                            if !existing.owns.iter().any(|o| o.name == own.name) {
                                existing.owns.push(own);
                            }
                        }
                        for order in entity.owns_order {
                            if !existing.owns_order.contains(&order) {
                                existing.owns_order.push(order);
                            }
                        }
                        for play in entity.plays {
                            if !existing.plays.iter().any(|p| p.role_ref == play.role_ref) {
                                existing.plays.push(play);
                            }
                        }
                    } else {
                        schema.entities.insert(name, entity);
                    }
                }
                Statement::Relation(relation) => {
                    let name = relation.name.clone();
                    if let Some(existing) = schema.relations.get_mut(&name) {
                        // Merge: TypeQL allows split definitions for the same type
                        if relation.parent.is_some() {
                            existing.parent = relation.parent;
                        }
                        if relation.is_abstract {
                            existing.is_abstract = true;
                        }
                        for own in relation.owns {
                            if !existing.owns.iter().any(|o| o.name == own.name) {
                                existing.owns.push(own);
                            }
                        }
                        for order in relation.owns_order {
                            if !existing.owns_order.contains(&order) {
                                existing.owns_order.push(order);
                            }
                        }
                        for play in relation.plays {
                            if !existing.plays.iter().any(|p| p.role_ref == play.role_ref) {
                                existing.plays.push(play);
                            }
                        }
                        for role in relation.roles {
                            if !existing.roles.iter().any(|r| r.name == role.name) {
                                existing.roles.push(role);
                            }
                        }
                    } else {
                        schema.relations.insert(name, relation);
                    }
                }
            }
        }
    }
    Ok(schema)
}

fn parse_define_block(input: &mut &str) -> PResult<Vec<Statement>> {
    ws_comments(input);
    literal("define").parse_next(input)?;
    ws_comments_required(input)?;
    let stmts: Vec<Option<Statement>> = repeat(1.., parse_statement).parse_next(input)?;
    Ok(stmts.into_iter().flatten().collect())
}

// ---------------------------------------------------------------------------
// Statement dispatch
// ---------------------------------------------------------------------------

enum Statement {
    Attribute(AttributeType),
    Entity(EntityType),
    Relation(RelationType),
}

fn parse_statement(input: &mut &str) -> PResult<Option<Statement>> {
    ws_comments(input);
    alt((
        parse_attribute_def.map(|a| Some(Statement::Attribute(a))),
        parse_entity_def.map(|e| Some(Statement::Entity(e))),
        parse_relation_def.map(|r| Some(Statement::Relation(r))),
        skip_function_def.map(|_| None),
        skip_struct_def.map(|_| None),
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Attribute definition
// ---------------------------------------------------------------------------

fn parse_attribute_def(input: &mut &str) -> PResult<AttributeType> {
    literal("attribute").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut attr = AttributeType {
        name: name.to_string(),
        value_type: String::new(),
        parent: None,
        is_abstract: false,
        is_independent: false,
        regex: None,
        allowed_values: None,
        range_min: None,
        range_max: None,
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_attribute_opt(input, &mut attr)?;
    }

    Ok(attr)
}

fn parse_attribute_opt<'a>(input: &mut &'a str, attr: &mut AttributeType) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            attr.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            attr.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@independent").parse_next(i)?;
            attr.is_independent = true;
            Ok(())
        },
        |i: &mut &'a str| {
            literal("value").parse_next(i)?;
            ws_comments_required(i)?;
            let vt = value_type_name(i)?;
            attr.value_type = vt.to_string();
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@regex").parse_next(i)?;
            ws_comments(i);
            let s = delimited("(", string_literal, ")").parse_next(i)?;
            attr.regex = Some(s);
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@values").parse_next(i)?;
            ws_comments(i);
            let vals: Vec<String> =
                delimited("(", separated(1.., padded(string_literal), ","), ")")
                    .parse_next(i)?;
            attr.allowed_values = Some(vals);
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@range").parse_next(i)?;
            ws_comments(i);
            let content: &str =
                delimited("(", take_until(0.., ")"), ")").parse_next(i)?;
            let content = content.trim();
            if let Some((min_s, max_s)) = content.split_once("..") {
                let min_s = min_s.trim();
                let max_s = max_s.trim();
                if !min_s.is_empty() {
                    attr.range_min = Some(min_s.to_string());
                }
                if !max_s.is_empty() {
                    attr.range_max = Some(max_s.to_string());
                }
            } else {
                // @range without ".." is invalid (e.g. @range(100) or @range(1, 5))
                return Err(ContextError::new());
            }
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Entity definition
// ---------------------------------------------------------------------------

fn parse_entity_def(input: &mut &str) -> PResult<EntityType> {
    literal("entity").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut entity = EntityType {
        name: name.to_string(),
        parent: None,
        is_abstract: false,
        owns: Vec::new(),
        owns_order: Vec::new(),
        plays: Vec::new(),
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_entity_clause(input, &mut entity)?;
    }

    Ok(entity)
}

fn parse_entity_clause<'a>(input: &mut &'a str, entity: &mut EntityType) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            entity.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            entity.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            let owned = parse_owns_statement(i)?;
            entity.owns_order.push(owned.name.clone());
            entity.owns.push(owned);
            Ok(())
        },
        |i: &mut &'a str| {
            let played = parse_plays_statement(i)?;
            entity.plays.push(played);
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Relation definition
// ---------------------------------------------------------------------------

fn parse_relation_def(input: &mut &str) -> PResult<RelationType> {
    literal("relation").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut relation = RelationType {
        name: name.to_string(),
        parent: None,
        is_abstract: false,
        roles: Vec::new(),
        owns: Vec::new(),
        owns_order: Vec::new(),
        plays: Vec::new(),
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_relation_clause(input, &mut relation)?;
    }

    Ok(relation)
}

fn parse_relation_clause<'a>(
    input: &mut &'a str,
    relation: &mut RelationType,
) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            relation.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            relation.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            let role = parse_relates_statement(i)?;
            relation.roles.push(role);
            Ok(())
        },
        |i: &mut &'a str| {
            let owned = parse_owns_statement(i)?;
            relation.owns_order.push(owned.name.clone());
            relation.owns.push(owned);
            Ok(())
        },
        |i: &mut &'a str| {
            let played = parse_plays_statement(i)?;
            relation.plays.push(played);
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Shared statement parsers: owns, plays, relates
// ---------------------------------------------------------------------------

fn parse_owns_statement(input: &mut &str) -> PResult<OwnedAttribute> {
    literal("owns").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut owned = OwnedAttribute {
        name: name.to_string(),
        is_key: false,
        is_unique: false,
        is_cascade: false,
        subkey_group: None,
        cardinality: None,
    };

    loop {
        ws_comments(input);
        let parsed = opt(alt((
            |i: &mut &str| {
                literal("@key").parse_next(i)?;
                owned.is_key = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@unique").parse_next(i)?;
                owned.is_unique = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@cascade").parse_next(i)?;
                owned.is_cascade = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@subkey").parse_next(i)?;
                ws_comments(i);
                let group: &str =
                    delimited("(", padded(identifier), ")").parse_next(i)?;
                owned.subkey_group = Some(group.to_string());
                Ok(())
            },
            |i: &mut &str| {
                let card = parse_card_annotation(i)?;
                owned.cardinality = Some(card);
                Ok(())
            },
        )))
        .parse_next(input)?;
        if parsed.is_none() {
            break;
        }
    }

    Ok(owned)
}

fn parse_plays_statement(input: &mut &str) -> PResult<PlayedRole> {
    literal("plays").parse_next(input)?;
    ws_comments_required(input)?;
    let relation = identifier(input)?;
    ws_comments(input);
    literal(":").parse_next(input)?;
    ws_comments(input);
    let role = identifier(input)?;

    let role_ref = format!("{}:{}", relation, role);

    ws_comments(input);
    let cardinality = opt(parse_card_annotation).parse_next(input)?;

    Ok(PlayedRole {
        role_ref,
        cardinality,
    })
}

fn parse_relates_statement(input: &mut &str) -> PResult<RoleSpec> {
    literal("relates").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut role = RoleSpec {
        name: name.to_string(),
        overrides: None,
        cardinality: None,
        distinct: false,
    };

    // Parse optional "as <parent_role>"
    ws_comments(input);
    if opt(literal("as")).parse_next(input)?.is_some() {
        ws_comments_required(input)?;
        let parent_role = identifier(input)?;
        role.overrides = Some(parent_role.to_string());
    }

    // Parse optional annotations
    loop {
        ws_comments(input);
        let parsed = opt(alt((
            |i: &mut &str| {
                literal("@distinct").parse_next(i)?;
                role.distinct = true;
                Ok(())
            },
            |i: &mut &str| {
                let card = parse_card_annotation(i)?;
                role.cardinality = Some(card);
                Ok(())
            },
        )))
        .parse_next(input)?;
        if parsed.is_none() {
            break;
        }
    }

    Ok(role)
}

// ---------------------------------------------------------------------------
// Annotation parsers
// ---------------------------------------------------------------------------

fn parse_card_annotation(input: &mut &str) -> PResult<Cardinality> {
    literal("@card").parse_next(input)?;
    ws_comments(input);
    literal("(").parse_next(input)?;
    ws_comments(input);
    let min = parse_u32(input)?;
    ws_comments(input);

    let has_dotdot = opt(literal("..")).parse_next(input)?;
    if has_dotdot.is_some() {
        ws_comments(input);
        let max = opt(parse_u32).parse_next(input)?;
        ws_comments(input);
        literal(")").parse_next(input)?;
        Ok(Cardinality { min, max })
    } else {
        ws_comments(input);
        literal(")").parse_next(input)?;
        Ok(Cardinality {
            min,
            max: Some(min),
        })
    }
}

// ---------------------------------------------------------------------------
// Function / Struct skip parsers
// ---------------------------------------------------------------------------

fn skip_function_def(input: &mut &str) -> PResult<()> {
    literal("fun").parse_next(input)?;
    ws_comments_required(input)?;
    // Skip everything until we find "return" followed by ";"
    loop {
        if input.is_empty() {
            return Err(ContextError::new());
        }
        if let Some(pos) = input.find("return") {
            *input = &input[pos + 6..];
            if let Some(semi_pos) = input.find(';') {
                *input = &input[semi_pos + 1..];
                return Ok(());
            }
        } else {
            if let Some(semi_pos) = input.find(';') {
                *input = &input[semi_pos + 1..];
                return Ok(());
            }
            return Err(ContextError::new());
        }
    }
}

fn skip_struct_def(input: &mut &str) -> PResult<()> {
    literal("struct").parse_next(input)?;
    ws_comments_required(input)?;
    let _: &str = terminated(take_until(0.., ";"), ";").parse_next(input)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Token-level parsers
// ---------------------------------------------------------------------------

/// Parse an identifier: `[a-zA-Z_][a-zA-Z0-9_-]*`
fn identifier<'a>(input: &mut &'a str) -> PResult<&'a str> {
    let ident: &str =
        take_while(1.., |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            .parse_next(input)?;

    let first = ident.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(ContextError::new());
    }

    Ok(ident)
}

/// Parse a value type name keyword.
fn value_type_name<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt((
        literal("datetime-tz"),
        literal("datetime"),
        literal("boolean"),
        literal("decimal"),
        literal("duration"),
        literal("integer"),
        literal("string"),
        literal("double"),
        literal("date"),
        literal("long"),
        literal("bool"),
        literal("int"),
    ))
    .parse_next(input)
}

/// Parse a quoted string literal (double or single quotes).
fn string_literal(input: &mut &str) -> PResult<String> {
    ws_comments(input);
    let quote: &str = alt((literal("\""), literal("'"))).parse_next(input)?;
    let close = if quote == "\"" { "\"" } else { "'" };

    let mut result = String::new();
    loop {
        if input.is_empty() {
            return Err(ContextError::new());
        }
        if input.starts_with(close) {
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
                '\'' => result.push('\''),
                other => {
                    // Preserve unknown escape sequences (e.g. \. in regex)
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

/// Parse an unsigned 32-bit integer.
fn parse_u32(input: &mut &str) -> PResult<u32> {
    let digits: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;
    digits.parse::<u32>().map_err(|_| ContextError::new())
}

// ---------------------------------------------------------------------------
// Whitespace / comment helpers
// ---------------------------------------------------------------------------

/// Skip whitespace and comments (# ... and // ...).
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

/// Skip an optional comma.
fn opt_comma(input: &mut &str) {
    let _ = opt(literal::<_, _, ContextError>(",")).parse_next(input);
}

/// Wrap a parser with surrounding whitespace/comment skipping.
fn padded<'a, O, P>(mut parser: P) -> impl FnMut(&mut &'a str) -> PResult<O>
where
    P: Parser<&'a str, O, ContextError>,
{
    move |input: &mut &'a str| {
        ws_comments(input);
        let result = parser.parse_next(input)?;
        ws_comments(input);
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identifier_simple() {
        let mut input = "person";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "person");
        assert_eq!(input, "");
    }

    #[test]
    fn test_identifier_with_hyphen() {
        let mut input = "isbn-13 rest";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "isbn-13");
        assert_eq!(input, " rest");
    }

    #[test]
    fn test_identifier_with_underscore() {
        let mut input = "_private_name";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "_private_name");
    }

    #[test]
    fn test_value_type_name() {
        for vt in &[
            "string", "integer", "double", "boolean", "date", "datetime",
            "datetime-tz", "decimal", "duration", "long", "bool", "int",
        ] {
            let mut input = *vt;
            let result = value_type_name(&mut input).unwrap();
            assert_eq!(result, *vt);
        }
    }

    #[test]
    fn test_string_literal_double_quotes() {
        let mut input = r#""hello world""#;
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_string_literal_single_quotes() {
        let mut input = "'hello'";
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_string_literal_with_escapes() {
        let mut input = r#""line1\nline2""#;
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn test_parse_u32() {
        let mut input = "42 rest";
        let result = parse_u32(&mut input).unwrap();
        assert_eq!(result, 42);
        assert_eq!(input, " rest");
    }

    #[test]
    fn test_card_annotation_bounded() {
        let mut input = "@card(1..3)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(card, Cardinality { min: 1, max: Some(3) });
    }

    #[test]
    fn test_card_annotation_unbounded() {
        let mut input = "@card(1..)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(card, Cardinality { min: 1, max: None });
    }

    #[test]
    fn test_card_annotation_exact() {
        let mut input = "@card(5)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(card, Cardinality { min: 5, max: Some(5) });
    }

    #[test]
    fn test_card_annotation_optional() {
        let mut input = "@card(0..1)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(card, Cardinality { min: 0, max: Some(1) });
    }

    // --- Attribute definition tests ---

    #[test]
    fn test_parse_simple_attribute() {
        let schema = parse_typeql("define\nattribute name, value string;").unwrap();
        assert_eq!(schema.attributes.len(), 1);
        let attr = schema.attributes.get("name").unwrap();
        assert_eq!(attr.value_type, "string");
        assert!(!attr.is_abstract);
    }

    #[test]
    fn test_parse_attribute_all_value_types() {
        for vt in &[
            "string", "integer", "double", "boolean", "date", "datetime",
            "decimal", "duration",
        ] {
            let input = format!("define\nattribute test-attr, value {};", vt);
            let schema = parse_typeql(&input).unwrap();
            let attr = schema.attributes.get("test-attr").unwrap();
            assert_eq!(attr.value_type, *vt);
        }
    }

    #[test]
    fn test_parse_attribute_with_regex() {
        let schema = parse_typeql(
            "define\nattribute email, value string, @regex(\"^[a-z]+@[a-z]+\\.[a-z]+$\");",
        )
        .unwrap();
        let attr = schema.attributes.get("email").unwrap();
        assert_eq!(attr.regex.as_deref(), Some("^[a-z]+@[a-z]+\\.[a-z]+$"));
    }

    #[test]
    fn test_parse_attribute_with_values() {
        let schema = parse_typeql(
            "define\nattribute status, value string, @values(\"active\", \"inactive\", \"pending\");",
        )
        .unwrap();
        let attr = schema.attributes.get("status").unwrap();
        assert_eq!(
            attr.allowed_values,
            Some(vec!["active".into(), "inactive".into(), "pending".into()])
        );
    }

    #[test]
    fn test_parse_attribute_with_range() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }

    #[test]
    fn test_parse_attribute_abstract() {
        let schema = parse_typeql("define\nattribute content @abstract;").unwrap();
        assert!(schema.attributes.get("content").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_attribute_independent() {
        let schema = parse_typeql("define\nattribute tag, value string, @independent;").unwrap();
        assert!(schema.attributes.get("tag").unwrap().is_independent);
    }

    #[test]
    fn test_parse_attribute_inheritance() {
        let schema = parse_typeql("define\nattribute isbn sub content, value string;").unwrap();
        let attr = schema.attributes.get("isbn").unwrap();
        assert_eq!(attr.parent.as_deref(), Some("content"));
        assert_eq!(attr.value_type, "string");
    }

    // --- Entity definition tests ---

    #[test]
    fn test_parse_simple_entity() {
        let schema = parse_typeql("define\nentity person;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.name, "person");
        assert!(!entity.is_abstract);
    }

    #[test]
    fn test_parse_entity_with_owns() {
        let schema = parse_typeql("define\nentity person, owns name, owns age;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.owns.len(), 2);
        assert_eq!(entity.owns_order, vec!["name", "age"]);
    }

    #[test]
    fn test_parse_entity_with_key() {
        let schema = parse_typeql("define\nentity person, owns name @key;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_key);
    }

    #[test]
    fn test_parse_entity_with_unique() {
        let schema = parse_typeql("define\nentity person, owns email @unique;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_unique);
    }

    #[test]
    fn test_parse_entity_with_cascade() {
        let schema = parse_typeql("define\nentity person, owns name @cascade;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_cascade);
    }

    #[test]
    fn test_parse_entity_with_subkey() {
        let schema = parse_typeql("define\nentity person, owns name @subkey(name_group);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().owns[0].subkey_group.as_deref(),
            Some("name_group")
        );
    }

    #[test]
    fn test_parse_entity_with_card() {
        let schema = parse_typeql("define\nentity person, owns nickname @card(0..3);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().owns[0].cardinality,
            Some(Cardinality { min: 0, max: Some(3) })
        );
    }

    #[test]
    fn test_parse_entity_with_plays() {
        let schema = parse_typeql("define\nentity person, plays friendship:friend;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.plays[0].role_ref, "friendship:friend");
    }

    #[test]
    fn test_parse_entity_with_plays_cardinality() {
        let schema =
            parse_typeql("define\nentity person, plays friendship:friend @card(0..5);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().plays[0].cardinality,
            Some(Cardinality { min: 0, max: Some(5) })
        );
    }

    #[test]
    fn test_parse_entity_abstract() {
        let schema = parse_typeql("define\nentity animal @abstract;").unwrap();
        assert!(schema.entities.get("animal").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_entity_inheritance() {
        let schema = parse_typeql("define\nentity employee sub person;").unwrap();
        assert_eq!(
            schema.entities.get("employee").unwrap().parent.as_deref(),
            Some("person")
        );
    }

    // --- Relation definition tests ---

    #[test]
    fn test_parse_simple_relation() {
        let schema = parse_typeql("define\nrelation friendship, relates friend;").unwrap();
        let rel = schema.relations.get("friendship").unwrap();
        assert_eq!(rel.roles.len(), 1);
        assert_eq!(rel.roles[0].name, "friend");
    }

    #[test]
    fn test_parse_relation_with_multiple_roles() {
        let schema =
            parse_typeql("define\nrelation employment, relates employer, relates employee;")
                .unwrap();
        assert_eq!(schema.relations.get("employment").unwrap().roles.len(), 2);
    }

    #[test]
    fn test_parse_relation_role_override() {
        let schema = parse_typeql(
            "define\nrelation authoring sub contribution, relates author as contributor;",
        )
        .unwrap();
        let rel = schema.relations.get("authoring").unwrap();
        assert_eq!(rel.roles[0].name, "author");
        assert_eq!(rel.roles[0].overrides.as_deref(), Some("contributor"));
    }

    #[test]
    fn test_parse_relation_role_distinct() {
        let schema = parse_typeql("define\nrelation team, relates member @distinct;").unwrap();
        assert!(schema.relations.get("team").unwrap().roles[0].distinct);
    }

    #[test]
    fn test_parse_relation_role_cardinality() {
        let schema =
            parse_typeql("define\nrelation friendship, relates friend @card(2..2);").unwrap();
        assert_eq!(
            schema.relations.get("friendship").unwrap().roles[0].cardinality,
            Some(Cardinality { min: 2, max: Some(2) })
        );
    }

    #[test]
    fn test_parse_relation_with_owns() {
        let schema = parse_typeql(
            "define\nrelation employment, relates employer, relates employee, owns start-date;",
        )
        .unwrap();
        let rel = schema.relations.get("employment").unwrap();
        assert_eq!(rel.owns[0].name, "start-date");
    }

    #[test]
    fn test_parse_relation_abstract() {
        let schema = parse_typeql(
            "define\nrelation contribution @abstract, relates contributor, relates work;",
        )
        .unwrap();
        assert!(schema.relations.get("contribution").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_relation_inheritance() {
        let schema = parse_typeql("define\nrelation authoring sub contribution;").unwrap();
        assert_eq!(
            schema.relations.get("authoring").unwrap().parent.as_deref(),
            Some("contribution")
        );
    }

    // --- Comments, whitespace, optional commas ---

    #[test]
    fn test_comments_ignored() {
        let schema = parse_typeql(
            "# comment\ndefine\n# another\nattribute name, value string;\n// cpp\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    #[test]
    fn test_block_comments_ignored() {
        let schema =
            parse_typeql("define\n/* block */\nattribute name, value string;").unwrap();
        assert_eq!(schema.attributes.len(), 1);
    }

    #[test]
    fn test_optional_commas() {
        let s1 = parse_typeql("define\nentity person owns name owns age;").unwrap();
        let s2 = parse_typeql("define\nentity person, owns name, owns age;").unwrap();
        assert_eq!(s1.entities.get("person").unwrap().owns.len(), 2);
        assert_eq!(s2.entities.get("person").unwrap().owns.len(), 2);
    }

    // --- Function/struct skip ---

    #[test]
    fn test_function_skipped() {
        let schema = parse_typeql(
            "define\nattribute name, value string;\nfun calc($d: date) -> integer:\n  match $x = 1;\n  return 30;\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    #[test]
    fn test_struct_skipped() {
        let schema = parse_typeql(
            "define\nattribute name, value string;\nstruct pn, value first string, value last string;\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    // --- Multiple define blocks ---

    #[test]
    fn test_multiple_define_blocks() {
        let schema = parse_typeql(
            "\ndefine\nattribute name, value string;\n\ndefine\nentity person, owns name;\n",
        )
        .unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    // --- Complex schema ---

    #[test]
    fn test_parse_complex_schema() {
        let schema = parse_typeql(
            "\
define

# Attributes
attribute name, value string;
attribute age, value integer;
attribute email, value string;
attribute isbn, value string;
attribute title, value string;
attribute pages, value integer;
attribute price, value double;
attribute published, value boolean;

# Entities
entity person @abstract
    , owns name @key
    , owns age
    , owns email @unique
    , plays friendship:friend
    , plays authoring:author;

entity employee sub person;

entity book @abstract
    , owns isbn @key
    , owns title
    , owns pages;

entity novel sub book
    , owns price;

# Relations
relation friendship
    , relates friend @card(2..2);

relation contribution @abstract
    , relates contributor
    , relates work;

relation authoring sub contribution
    , relates author as contributor
    , relates book as work;
",
        )
        .unwrap();

        assert_eq!(schema.attributes.len(), 8);
        assert_eq!(schema.entities.len(), 4);
        assert_eq!(schema.relations.len(), 3);

        let person = schema.entities.get("person").unwrap();
        assert!(person.is_abstract);
        assert_eq!(person.owns.len(), 3);
        assert!(person.owns.iter().any(|o| o.name == "name" && o.is_key));
        assert!(person.owns.iter().any(|o| o.name == "email" && o.is_unique));
        assert_eq!(person.plays.len(), 2);
    }

    // --- Error handling ---

    #[test]
    fn test_parse_error_no_define() {
        assert!(parse_typeql("attribute name, value string;").is_err());
    }

    #[test]
    fn test_parse_error_trailing_content() {
        assert!(parse_typeql("define\nattribute name, value string;\ngarbage").is_err());
    }

    // --- Full round-trip with inheritance ---

    #[test]
    fn test_parse_and_resolve() {
        let schema = TypeSchema::from_typeql(
            "\
define
attribute name, value string;
attribute age, value integer;

entity person
    , owns name @key
    , owns age
    , plays friendship:friend;

entity employee sub person;

relation friendship
    , relates friend @card(2..2);
",
        )
        .unwrap();

        let employee = schema.get_entity("employee").unwrap();
        assert_eq!(employee.owns.len(), 2);
        assert_eq!(employee.plays.len(), 1);

        let attrs = schema.get_all_owned_attributes("employee");
        assert_eq!(attrs.len(), 2);

        let roles = schema.get_all_plays_roles("employee");
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].role_ref, "friendship:friend");
    }

    #[test]
    fn test_empty_input() {
        let schema = parse_typeql("").unwrap();
        assert!(schema.entities.is_empty());
    }

    #[test]
    fn test_only_comments() {
        let schema = parse_typeql("# comment\n// another\n").unwrap();
        assert!(schema.entities.is_empty());
    }

    #[test]
    fn test_parse_multiline_entity() {
        let schema = parse_typeql(
            "\
define
entity person
    , owns name @key
    , owns age @card(0..1)
    , owns email @unique
    , plays friendship:friend @card(0..10)
    , plays employment:employee;
",
        )
        .unwrap();
        let person = schema.entities.get("person").unwrap();
        assert_eq!(person.owns.len(), 3);
        assert_eq!(person.plays.len(), 2);
        assert!(person.owns[0].is_key);
        assert_eq!(person.owns[1].cardinality, Some(Cardinality { min: 0, max: Some(1) }));
        assert!(person.owns[2].is_unique);
        assert_eq!(person.plays[0].cardinality, Some(Cardinality { min: 0, max: Some(10) }));
        assert!(person.plays[1].cardinality.is_none());
    }

    // --- @range validation tests ---

    #[test]
    fn test_range_no_dotdot_fails() {
        let result = parse_typeql("define\nattribute age, value integer, @range(100);");
        assert!(result.is_err());
    }

    #[test]
    fn test_range_comma_fails() {
        let result = parse_typeql("define\nattribute age, value integer, @range(1, 5);");
        assert!(result.is_err());
    }

    #[test]
    fn test_range_valid_passes() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }

    #[test]
    fn test_range_open_ended_min() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert!(attr.range_max.is_none());
    }

    #[test]
    fn test_range_open_ended_max() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert!(attr.range_min.is_none());
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }
}
