//! Active schema parsing through the official TypeQL adapter.
use crate::_schema::{SchemaError, TypeSchema};

/// Parse and resolve a schema using the official TypeQL grammar.
pub fn parse_schema(source: &str) -> Result<TypeSchema, SchemaError> {
    // Empty exports are the schema API sentinel, not a grammar alternative.
    if source.trim().is_empty() || source.trim() == "define" {
        return Ok(TypeSchema::new());
    }
    crate::tql::project_schema(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_schema::SchemaError;

    #[test]
    fn official_parser_resolves_inheritance_and_preserves_annotations() {
        let schema=parse_schema(r#"define attribute name, value string; entity base @abstract, owns name @key @doc("name") @meta("pii", "yes"); entity person, sub base; relation team, relates member[] @distinct @card(0..3); person, plays team:member @card(0..1);"#).expect("official schema");
        let person = &schema.entities["person"];
        assert!(person.owns[0].is_key);
        assert_eq!(person.owns[0].doc.as_deref(), Some("name"));
        assert_eq!(person.owns[0].meta["pii"], "yes");
        assert_eq!(person.plays[0].cardinality.as_ref().unwrap().max, Some(1));
        assert!(schema.relations["team"].roles[0].ordered);
        assert!(schema.relations["team"].roles[0].distinct);
    }
    #[test]
    fn official_parser_keeps_function_and_struct_signatures() {
        let schema=parse_schema("define fun names($p: person) -> { string }: match $p has name $n; return { $n }; struct address:street value string, zip value integer?;").expect("schema signatures");
        assert!(schema.functions["names"].return_type.is_stream);
        assert_eq!(schema.functions["names"].parameters[0].name, "p");
        assert_eq!(
            schema.functions["names"].return_type.types[0].name,
            "string"
        );
        assert!(schema.structs["address"].fields[1].optional);
    }
    #[test]
    fn official_parser_reports_unrepresentable_constructs_without_dropping_them() {
        for source in [
            "define entity person, alias human;",
            "define attribute age, value integer @values(1, 2);",
            "define fun f() -> integer @doc(\"doc\"): match let $one = 1; return first $one;",
            "match $p isa person;",
        ] {
            assert!(
                matches!(
                    parse_schema(source),
                    Err(SchemaError::UnsupportedProjection { .. })
                ),
                "{source}"
            );
        }
    }
    #[test]
    fn official_parser_rejects_released_only_syntax_but_archive_reader_retains_it() {
        let source = "define attribute name value string; entity person @abstract owns name;";
        assert!(matches!(
            parse_schema(source),
            Err(SchemaError::ParseError { .. })
        ));
        #[allow(deprecated)]
        crate::_parser::parse_typeql(source).expect("released archive remains readable");
    }
}
