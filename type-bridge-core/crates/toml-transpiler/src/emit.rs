//! Checked schema construction through the shared official-AST adapter.
use crate::model::{TomlOwns, TomlPlays, TomlSchema};
use indexmap::IndexMap;
use type_bridge_core_lib::__internal as tql;

// TOML accepts these historical aliases; TypeDB requires builtin value keywords.
fn value_domain(domain: &str) -> &str {
    match domain {
        "long" | "int" => "integer",
        "bool" => "boolean",
        other => other,
    }
}

fn doc_meta(
    output: &mut Vec<tql::SchemaAnnotation>,
    doc: Option<&str>,
    meta: &IndexMap<String, String>,
) -> Result<(), tql::TqlError> {
    if let Some(doc) = doc {
        output.push(tql::schema_doc(doc)?);
    }
    for (key, value) in meta {
        output.push(tql::schema_meta(key, value)?);
    }
    Ok(())
}
fn head(
    abstract_: bool,
    doc: Option<&str>,
    meta: &IndexMap<String, String>,
) -> Result<Vec<tql::SchemaAnnotation>, tql::TqlError> {
    let mut output = Vec::new();
    if abstract_ {
        output.push(tql::schema_flag("abstract")?);
    }
    doc_meta(&mut output, doc, meta)?;
    Ok(output)
}
fn metadata(annotations: &[String], case: Option<&str>) -> Vec<String> {
    let mut output = annotations.to_vec();
    if let Some(case) = case {
        output.push(format!("case({case})"));
    }
    output
}
fn parent(
    output: &mut Vec<tql::SchemaCapability>,
    name: Option<&str>,
) -> Result<(), tql::TqlError> {
    if let Some(name) = name {
        output.push(tql::schema_sub(name)?);
    }
    Ok(())
}
fn owns(entry: &TomlOwns) -> Result<tql::SchemaCapability, tql::TqlError> {
    match entry {
        TomlOwns::Name(name) => tql::schema_owns(name, false, vec![]),
        TomlOwns::Annotated(entry) => {
            let mut annotations = Vec::new();
            for (name, present) in [
                ("key", entry.key),
                ("unique", entry.unique),
                ("distinct", entry.distinct),
            ] {
                if present {
                    annotations.push(tql::schema_flag(name)?);
                }
            }
            if let Some(card) = &entry.card {
                annotations.push(tql::schema_annotation("card", Some(card))?);
            }
            doc_meta(&mut annotations, entry.doc.as_deref(), &entry.meta)?;
            tql::schema_owns(&entry.attribute, entry.ordered, annotations)
        }
    }
}
fn plays(entry: &TomlPlays) -> Result<tql::SchemaCapability, tql::TqlError> {
    let annotations = entry
        .card
        .as_deref()
        .map(|card| tql::schema_annotation("card", Some(card)))
        .transpose()?
        .into_iter()
        .collect();
    tql::schema_plays(&entry.relation, &entry.role, annotations)
}
/// Emit checked official schema nodes in TOML declaration order.
pub fn emit(schema: &TomlSchema) -> Result<String, tql::TqlError> {
    let mut definitions = Vec::new();
    for (name, attribute) in &schema.attributes {
        let mut capabilities = Vec::new();
        parent(&mut capabilities, attribute.sub.as_deref())?;
        if let Some(domain) = &attribute.value {
            let mut annotations = Vec::new();
            if let Some(regex) = &attribute.regex {
                annotations.push(tql::schema_regex_source(regex)?);
            }
            if let Some(values) = &attribute.values {
                annotations.push(tql::schema_values_source(
                    &values.iter().map(String::as_str).collect::<Vec<_>>(),
                )?);
            }
            if let Some(range) = &attribute.range {
                annotations.push(tql::schema_annotation("range", Some(range))?);
            }
            capabilities.push(tql::schema_value(value_domain(domain), annotations)?);
        }
        definitions.push((
            tql::schema_type(
                Some("attribute"),
                name,
                head(
                    attribute.is_abstract,
                    attribute.doc.as_deref(),
                    &attribute.meta,
                )?,
                capabilities,
            )?,
            metadata(&attribute.annotations, attribute.bindgen_case.as_deref()),
        ));
    }
    for (name, entity) in &schema.entities {
        let mut capabilities = Vec::new();
        parent(&mut capabilities, entity.sub.as_deref())?;
        for entry in &entity.owns {
            capabilities.push(owns(entry)?);
        }
        for entry in &entity.plays {
            capabilities.push(plays(entry)?);
        }
        definitions.push((
            tql::schema_type(
                Some("entity"),
                name,
                head(entity.is_abstract, entity.doc.as_deref(), &entity.meta)?,
                capabilities,
            )?,
            metadata(&entity.annotations, entity.bindgen_case.as_deref()),
        ));
    }
    for (name, relation) in &schema.relations {
        let mut capabilities = Vec::new();
        parent(&mut capabilities, relation.sub.as_deref())?;
        for role in &relation.roles {
            let mut annotations = Vec::new();
            if role.is_abstract {
                annotations.push(tql::schema_flag("abstract")?);
            }
            if role.distinct {
                annotations.push(tql::schema_flag("distinct")?);
            }
            if let Some(card) = &role.card {
                annotations.push(tql::schema_annotation("card", Some(card))?);
            }
            doc_meta(&mut annotations, role.doc.as_deref(), &role.meta)?;
            capabilities.push(tql::schema_relates(
                &role.name,
                role.ordered,
                role.overrides.as_deref(),
                annotations,
            )?);
        }
        for entry in &relation.owns {
            capabilities.push(owns(entry)?);
        }
        for entry in &relation.plays {
            capabilities.push(plays(entry)?);
        }
        definitions.push((
            tql::schema_type(
                Some("relation"),
                name,
                head(
                    relation.is_abstract,
                    relation.doc.as_deref(),
                    &relation.meta,
                )?,
                capabilities,
            )?,
            metadata(&relation.annotations, relation.bindgen_case.as_deref()),
        ));
    }
    for function in schema.functions.values() {
        definitions.push((
            tql::schema_function(&function.signature, &function.body)?,
            vec![],
        ));
    }
    for (name, structure) in &schema.structs {
        definitions.push((
            tql::schema_struct(
                name,
                structure
                    .fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.as_str(),
                            value_domain(&field.value_type),
                            field.optional,
                        )
                    })
                    .collect(),
            )?,
            vec![],
        ));
    }
    tql::render_schema_document(definitions)
}

#[cfg(test)]
mod tests {
    use crate::toml_to_typeql;

    // -------------------------------------------------------------------------
    // 01/02 back-compat regression guard
    // -------------------------------------------------------------------------

    /// Feed the canonical attribute+entity slice TOML through `toml_to_typeql`
    /// and verify the emitted TypeQL contains all expected declarations in order.
    #[test]
    fn test_emit_smoke() {
        let toml_text = r#"
[attributes.name]
value = "string"

[attributes.age]
value = "long"

[entities.person]
owns = ["name", "age"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(result.contains("define"), "output must contain `define`");
        assert!(
            result.contains("attribute name, value string;"),
            "output must contain attribute name declaration; got:\n{result}"
        );
        assert!(
            result.contains("attribute age, value integer;"),
            "output must contain attribute age declaration; got:\n{result}"
        );
        assert!(
            result.contains("entity person, owns name, owns age;"),
            "output must contain entity person declaration; got:\n{result}"
        );

        // Verify owns order: "name" must appear before "age" in the output.
        let name_pos = result
            .find("owns name")
            .expect("owns name not found in output");
        let age_pos = result
            .find("owns age")
            .expect("owns age not found in output");
        assert!(
            name_pos < age_pos,
            "`owns name` must appear before `owns age`; got:\n{result}"
        );
    }

    /// A relation with an empty `roles` array emits `relation N;`.
    #[test]
    fn test_emit_empty_roles() {
        let toml_text = r#"
[relations.empty_rel]
roles = []
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relation empty_rel;"),
            "expected `relation empty_rel;`; got:\n{result}"
        );
    }

    /// A role with no `card` or `as` emits a bare `relates <name>` (no
    /// `@card(...)`, no `as ...`).
    #[test]
    fn test_emit_bare_relates() {
        let toml_text = r#"
[relations.bare_rel]
roles = [{ name = "participant" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relates participant"),
            "expected bare `relates participant`; got:\n{result}"
        );
        assert!(
            !result.contains("@card"),
            "bare role must not emit @card; got:\n{result}"
        );
        assert!(
            !result.contains(" as "),
            "bare role must not emit `as`; got:\n{result}"
        );
    }

    /// A role with an `as` override emits `relates <name> as <target>`.
    #[test]
    fn test_emit_role_as_override() {
        let toml_text = r#"
[relations.authoring]
roles = [{ name = "author", as = "contributor" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relates author as contributor"),
            "expected `relates author as contributor`; got:\n{result}"
        );
    }

    /// Two roles declared in a specific array order must appear in that same
    /// order in the emitted TypeQL.
    #[test]
    fn test_emit_roles_order_preserved() {
        let toml_text = r#"
[relations.ordered_rel]
roles = [
    { name = "first" },
    { name = "second" },
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        let first_pos = result
            .find("relates first")
            .expect("`relates first` not found");
        let second_pos = result
            .find("relates second")
            .expect("`relates second` not found");
        assert!(
            first_pos < second_pos,
            "`relates first` must appear before `relates second`; got:\n{result}"
        );
    }

    /// Integration smoke: a full relation+plays TOML feeds through `toml_to_typeql`
    /// and the output contains the expected relation and plays declarations.
    #[test]
    fn test_emit_relation_and_plays_smoke() {
        let toml_text = r#"
[attributes.name]
value = "string"

[attributes.score]
value = "double"

[entities.person]
owns = ["name"]
plays = [{ relation = "review", role = "reviewer" }]

[entities.document]
owns = ["name"]
plays = [{ relation = "review", role = "document" }]

[relations.review]
roles = [
    { name = "document", card = "1..1" },
    { name = "reviewer", card = "1..3" },
]
owns = ["score"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(
            result.contains("relation review, relates document @card(1..1), relates reviewer @card(1..3), owns score;"),
            "expected full review relation declaration; got:\n{result}"
        );
        assert!(
            result.contains("plays review:reviewer"),
            "expected `plays review:reviewer` on person; got:\n{result}"
        );
        assert!(
            result.contains("plays review:document"),
            "expected `plays review:document` on document entity; got:\n{result}"
        );
    }

    /// Entity plays entries can carry plays-side cardinality.
    #[test]
    fn test_emit_entity_plays_cardinality() {
        let toml_text = r#"
[entities.post]
plays = [
    { relation = "posting", role = "post", card = "1" },
    { relation = "reaction", role = "parent", card = "0..5" },
    { relation = "commenting", role = "parent", card = "1.." },
]

[relations.posting]
roles = [{ name = "post" }]

[relations.reaction]
roles = [{ name = "parent" }]

[relations.commenting]
roles = [{ name = "parent" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(
            result.contains("plays posting:post @card(1)"),
            "expected exact plays @card; got:\n{result}"
        );
        assert!(
            result.contains("plays reaction:parent @card(0..5)"),
            "expected bounded plays @card; got:\n{result}"
        );
        assert!(
            result.contains("plays commenting:parent @card(1..)"),
            "expected unbounded plays @card; got:\n{result}"
        );
    }

    /// Relations can play roles in other relations; relation clauses emit
    /// after relates and owns clauses.
    #[test]
    fn test_emit_relation_level_plays() {
        let toml_text = r#"
[attributes.title]
value = "string"

[relations.publication]
roles = [{ name = "publisher" }]
owns = ["title"]
plays = [
    { relation = "contribution", role = "work" },
    { relation = "review", role = "reviewed", card = "0..5" },
]

[relations.contribution]
roles = [{ name = "work" }]

[relations.review]
roles = [{ name = "reviewed" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(
            result.contains("relation publication, relates publisher, owns title, plays contribution:work, plays review:reviewed @card(0..5);"),
            "expected relation-level plays after roles and owns; got:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // attribute abstract / sub / value-annotation emission
    // -------------------------------------------------------------------------

    /// `abstract = true` on an attribute emits `attribute X @abstract, value T;`.
    #[test]
    fn test_emit_attribute_abstract() {
        let toml_text = r#"
[attributes.isbn]
value = "string"
abstract = true
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute isbn @abstract, value string;"),
            "expected `attribute isbn @abstract, value string;`; got:\n{result}"
        );
    }

    /// `sub = "isbn"` with no `value` emits `attribute isbn-13, sub isbn;` (no `value` token).
    #[test]
    fn test_emit_attribute_sub() {
        let toml_text = r#"
[attributes.isbn]
value = "string"

[attributes.isbn-13]
sub = "isbn"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute isbn-13, sub isbn;"),
            "expected `attribute isbn-13, sub isbn;`; got:\n{result}"
        );
        assert!(
            !result.contains("isbn-13 sub isbn,"),
            "sub attribute must NOT emit a `value` clause; got:\n{result}"
        );
    }

    /// `abstract = true` and `sub = "..."` must both be emitted when present.
    #[test]
    fn test_emit_abstract_subtype_heads() {
        let toml_text = r#"
[attributes.payload]
value = "string"

[attributes.text-payload]
abstract = true
sub = "payload"

[entities.content]
abstract = true

[entities.page]
abstract = true
sub = "content"

[relations.interaction]
abstract = true

[relations.content-engagement]
abstract = true
sub = "interaction"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        assert!(
            result.contains("attribute text-payload @abstract, sub payload;"),
            "expected abstract sub-attribute; got:\n{result}"
        );
        assert!(
            result.contains("entity page @abstract, sub content;"),
            "expected abstract sub-entity; got:\n{result}"
        );
        assert!(
            result.contains("relation content-engagement @abstract, sub interaction;"),
            "expected abstract sub-relation; got:\n{result}"
        );
    }

    /// `regex = "..."` emits `@regex("...")` after `value`.
    #[test]
    fn test_emit_attribute_regex() {
        let toml_text = r#"
[attributes.status]
value = "string"
regex = "^(paid|dispatched)$"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(r#"attribute status, value string @regex("^(paid|dispatched)$");"#),
            "expected @regex after value; got:\n{result}"
        );
    }

    /// Explicit `annotations` and `bindgen_case` emit correctly.
    #[test]
    fn test_emit_annotations_and_bindgen_case() {
        let toml_text = r#"
[attributes.my-attr]
value = "string"
bindgen_case = "PascalCase"
annotations = ["internal"]

[entities.my-entity]
bindgen_case = "Python, SnakeCase"
annotations = ["dto_name(MyEntityDto)"]

[relations.my-relation]
bindgen_case = "CamelCase"
annotations = ["custom_annotation"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        assert!(
            result.contains("# @internal\n# @case(PascalCase)\nattribute my-attr"),
            "expected attr annotations; got:\n{result}"
        );
        assert!(
            result
                .contains("# @dto_name(MyEntityDto)\n# @case(Python, SnakeCase)\nentity my-entity"),
            "expected entity annotations; got:\n{result}"
        );
        assert!(
            result.contains("# @custom_annotation\n# @case(CamelCase)\nrelation my-relation"),
            "expected relation annotations; got:\n{result}"
        );
    }

    /// `values = [...]` emits `@values("a", "b", ...)` after `value`.
    #[test]
    fn test_emit_attribute_values() {
        let toml_text = r#"
[attributes.reaction]
value = "string"
values = ["like", "love", "funny"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result
                .contains(r#"attribute reaction, value string @values("like", "love", "funny");"#),
            "expected @values after value; got:\n{result}"
        );
    }

    /// `range = "0..150"` emits `@range(0..150)` after `value`.
    #[test]
    fn test_emit_attribute_range() {
        let toml_text = r#"
[attributes.age]
value = "integer"
range = "0..150"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute age, value integer @range(0..150);"),
            "expected @range after value; got:\n{result}"
        );
    }

    /// `entity X @abstract` and `entity Y, sub X` both emit correctly.
    #[test]
    fn test_emit_entity_abstract_and_sub() {
        let toml_text = r#"
[entities.book]
abstract = true

[entities.hardback]
sub = "book"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("entity book @abstract;"),
            "expected `entity book @abstract;`; got:\n{result}"
        );
        assert!(
            result.contains("entity hardback, sub book;"),
            "expected `entity hardback, sub book;`; got:\n{result}"
        );
    }

    /// `relation authoring, sub contribution, ...` emits correctly.
    #[test]
    fn test_emit_relation_sub() {
        let toml_text = r#"
[relations.contribution]
roles = [{ name = "contributor" }, { name = "work" }]

[relations.authoring]
sub = "contribution"
roles = [{ name = "author", as = "contributor" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("relation authoring, sub contribution,"),
            "expected `relation authoring, sub contribution,`; got:\n{result}"
        );
        assert!(
            result.contains("relates author as contributor"),
            "expected `relates author as contributor`; got:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // Function and struct emission
    // -------------------------------------------------------------------------

    /// A stream-return function emits `<signature>:\n<body>` — the emitter
    /// parses the signature and body into official function nodes.
    #[test]
    fn test_emit_function_passthrough() {
        let toml_text = r#"
[functions.f]
signature = "fun f($x: user) -> { book }"
body = "  match $x isa user;\n  return { $b };"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        // The emitter must append `:` immediately after the signature.
        assert!(
            result.contains("fun f($x: user) -> { book }:"),
            "output must contain the signature with emitter-appended colon; got:\n{result}"
        );
        // The verbatim body must be present.
        assert!(
            result.contains("return"),
            "output must contain the verbatim body with `return`; got:\n{result}"
        );
        assert!(
            result.contains(';'),
            "output must contain the trailing `;` from the body; got:\n{result}"
        );
    }

    /// A scalar-return function (`-> double`, no braces) emits with the
    /// emitter-appended `:` as well.
    #[test]
    fn test_emit_function_scalar_return() {
        let toml_text = r#"
[functions.best-price]
signature = "fun best-price($line: order-line) -> double"
body = "  match $line has price $p;\n  return max($p);"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("fun best-price($line: order-line) -> double:"),
            "scalar-return signature must have the emitter-appended colon; got:\n{result}"
        );
    }

    /// Two non-optional struct fields emit as
    /// `struct person-name:first-name value string, last-name value string;`.
    #[test]
    fn test_emit_struct_basic() {
        let toml_text = r#"
[structs.person-name]
fields = [
    { name = "first-name", type = "string" },
    { name = "last-name", type = "string" },
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("struct person-name:first-name value string, last-name value string;"),
            "expected two-field struct declaration; got:\n{result}"
        );
    }

    /// An optional struct field emits a `?` suffix on the type name.
    #[test]
    fn test_emit_struct_optional_field() {
        let toml_text = r#"
[structs.name]
fields = [
    { name = "first", type = "string" },
    { name = "m", type = "long", optional = true },
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("m value integer?"),
            "optional field must emit `<type>?` suffix; got:\n{result}"
        );
        assert!(
            !result.contains("value first string?"),
            "non-optional field must not emit `?`; got:\n{result}"
        );
    }

    /// A TOML without `[functions]` or `[structs]` sections deserializes and
    /// emits without any function or struct lines — back-compat with 01/02/03 fixtures.
    #[test]
    fn test_functions_structs_default_absent() {
        let toml_text = r#"
[attributes.name]
value = "string"

[entities.person]
owns = ["name"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            !result.contains("fun "),
            "output must contain no function declarations; got:\n{result}"
        );
        assert!(
            !result.contains("struct "),
            "output must contain no struct declarations; got:\n{result}"
        );
        // The existing attribute and entity declarations must still be present.
        assert!(
            result.contains("attribute name, value string;"),
            "attribute declaration must still emit; got:\n{result}"
        );
        assert!(
            result.contains("entity person, owns name;"),
            "entity declaration must still emit; got:\n{result}"
        );
    }

    /// Integration smoke: a TOML with one function and one struct feeds through
    /// `toml_to_typeql` end-to-end — the output contains the function head with
    /// the signature, parsed body, and corrected struct declaration.
    #[test]
    fn test_emit_p0_functions_structs_smoke() {
        let toml_text = r#"
[attributes.name]
value = "string"

[entities.user]
owns = ["name"]

[functions.count-users]
signature = "fun count-users($u: user) -> { integer }"
body = "  match $u isa user;\n  let $one = 1; return { $one };"

[structs.person-name]
fields = [
    { name = "first-name", type = "string" },
    { name = "middle-name", type = "string", optional = true },
    { name = "last-name", type = "string" },
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        // Function: signature + emitter-appended colon
        assert!(
            result.contains("fun count-users($u: user) -> { integer }:"),
            "function head must carry the emitter-appended colon; got:\n{result}"
        );
        // Function: parsed body contains `return`
        assert!(
            result.contains("return"),
            "parsed body must contain `return`; got:\n{result}"
        );
        // Struct: standard two-field form + optional field with `?`
        assert!(
            result.contains("struct person-name:"),
            "struct declaration must be present; got:\n{result}"
        );
        assert!(
            result.contains("middle-name value string?"),
            "optional struct field must emit `<type>?`; got:\n{result}"
        );
        assert!(
            result.contains("first-name value string"),
            "non-optional struct field must emit without `?`; got:\n{result}"
        );
    }

    /// Integration smoke: feeds abstract/sub/regex TOML through `toml_to_typeql`
    /// and asserts the output contains expected declarations.
    #[test]
    fn test_emit_p0_integration_smoke() {
        let toml_text = r#"
[attributes.isbn]
value = "string"
abstract = true

[attributes.isbn-13]
sub = "isbn"

[attributes.status]
value = "string"
regex = "^(paid|dispatched|delivered)$"

[entities.book]
abstract = true
owns = [{ attribute = "isbn-13", key = true }]

[entities.hardback]
sub = "book"
owns = ["stock"]

[attributes.stock]
value = "integer"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        assert!(
            result.contains("attribute isbn @abstract, value string;"),
            "expected `attribute isbn @abstract, value string;`; got:\n{result}"
        );
        assert!(
            result.contains("attribute isbn-13, sub isbn;"),
            "expected `attribute isbn-13, sub isbn;`; got:\n{result}"
        );
        assert!(
            result.contains("entity book @abstract"),
            "expected `entity book @abstract`; got:\n{result}"
        );
        assert!(
            result.contains("entity hardback, sub book"),
            "expected `entity hardback, sub book`; got:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // owns-level annotation surface (@key / @unique / @card on owned attributes)
    // -------------------------------------------------------------------------

    /// `{ attribute = "isbn-13", key = true }` emits `owns isbn-13 @key`.
    #[test]
    fn test_emit_owns_key() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn-13", key = true }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-13 @key"),
            "expected `owns isbn-13 @key`; got:\n{result}"
        );
    }

    /// `{ attribute = "isbn-10", unique = true }` emits `owns isbn-10 @unique`.
    #[test]
    fn test_emit_owns_unique() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn-10", unique = true }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-10 @unique"),
            "expected `owns isbn-10 @unique`; got:\n{result}"
        );
    }

    /// `{ attribute = "isbn", card = "0..2" }` emits `owns isbn @card(0..2)`.
    #[test]
    fn test_emit_owns_card() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn", card = "0..2" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "expected `owns isbn @card(0..2)`; got:\n{result}"
        );
    }

    /// Mixed owns array: annotated table + bare string must emit in array order.
    #[test]
    fn test_emit_owns_mixed() {
        let toml_text = r#"
[entities.book]
owns = [
    { attribute = "isbn-13", key = true },
    { attribute = "isbn-10", unique = true },
    { attribute = "isbn", card = "0..2" },
    "title",
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        // All four present
        assert!(
            result.contains("owns isbn-13 @key"),
            "missing `owns isbn-13 @key`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn-10 @unique"),
            "missing `owns isbn-10 @unique`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "missing `owns isbn @card(0..2)`; got:\n{result}"
        );
        assert!(
            result.contains("owns title"),
            "missing `owns title`; got:\n{result}"
        );

        // Order preserved
        let pos_key = result.find("owns isbn-13 @key").unwrap();
        let pos_unique = result.find("owns isbn-10 @unique").unwrap();
        let pos_card = result.find("owns isbn @card(0..2)").unwrap();
        let pos_title = result.find("owns title").unwrap();
        assert!(
            pos_key < pos_unique,
            "isbn-13 @key must precede isbn-10 @unique"
        );
        assert!(
            pos_unique < pos_card,
            "isbn-10 @unique must precede isbn @card"
        );
        assert!(pos_card < pos_title, "isbn @card must precede title");
    }

    /// Integration smoke: TOML with annotated owns feeds through correctly.
    #[test]
    fn test_emit_p1_integration_smoke() {
        let toml_text = r#"
[entities.book]
owns = [
    { attribute = "isbn-13", key = true },
    { attribute = "isbn", card = "0..2" },
    "title",
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-13 @key"),
            "missing `owns isbn-13 @key`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "missing `owns isbn @card(0..2)`; got:\n{result}"
        );
        assert!(
            result.contains("owns title"),
            "missing `owns title`; got:\n{result}"
        );

        // Order: isbn-13 @key < isbn @card < title
        let k = result.find("owns isbn-13 @key").unwrap();
        let c = result.find("owns isbn @card(0..2)").unwrap();
        let t = result.find("owns title").unwrap();
        assert!(k < c && c < t, "owns order not preserved; got:\n{result}");
    }

    #[test]
    fn test_emit_doc_meta_on_type_heads() {
        let toml_text = r##"
[attributes.name]
value = "string"
doc = "a personal name"
meta = { pii = "true" }

[entities.person]
doc = "an individual client"
meta = { icon = "silhouette.png" }
owns = ["name"]

[relations.friendship]
doc = "a mutual bond"
roles = [{ name = "friend" }]
"##;
        let result = crate::toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(
                r#"attribute name @doc("a personal name") @meta("pii", "true"), value string;"#
            ),
            "attribute head wrong; got:\n{result}"
        );
        assert!(
            result.contains(
                r#"entity person @doc("an individual client") @meta("icon", "silhouette.png"), owns name;"#
            ),
            "entity head wrong; got:\n{result}"
        );
        assert!(
            result.contains(r#"relation friendship @doc("a mutual bond"), relates friend;"#),
            "relation head wrong; got:\n{result}"
        );
    }

    #[test]
    fn test_emit_doc_meta_head_before_sub_uses_comma() {
        let toml_text = r##"
[entities.base]
abstract = true

[entities.child]
sub = "base"
doc = "d"
"##;
        let result = crate::toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(r#"entity child @doc("d"), sub base;"#),
            "child head wrong; got:\n{result}"
        );
    }

    #[test]
    fn test_emit_doc_meta_on_owns_and_roles() {
        let toml_text = r##"
[attributes.name]
value = "string"

[entities.person]
owns = [{ attribute = "name", key = true, doc = "full legal name", meta = { column = "name" } }]

[relations.friendship]
roles = [{ name = "friend", card = "0..2", doc = "one side", meta = { endpoint = "true" } }]
"##;
        let result = crate::toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(r#"owns name @key @doc("full legal name") @meta("column", "name")"#),
            "owns clause wrong; got:\n{result}"
        );
        assert!(
            result.contains(
                r#"relates friend @card(0..2) @doc("one side") @meta("endpoint", "true")"#
            ),
            "relates clause wrong; got:\n{result}"
        );
    }

    #[test]
    fn test_emit_meta_preserves_declaration_order_and_escapes() {
        let toml_text = r##"
[entities.gadget]
doc = """line1
line2 with "quotes""""
meta = { z-key = "1", a-key = "2" }
"##;
        let result = crate::toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(r#"@doc("line1\nline2 with \"quotes\"")"#),
            "doc escaping wrong; got:\n{result}"
        );
        let z = result
            .find(r#"@meta("z-key", "1")"#)
            .expect("z-key missing");
        let a = result
            .find(r#"@meta("a-key", "2")"#)
            .expect("a-key missing");
        assert!(
            z < a,
            "meta must keep TOML declaration order; got:\n{result}"
        );
    }
}
