//! Tokenizer for ownership flag strings (`@key @card(1..5) @doc("...")`).
//!
//! Flag strings are produced by [`super::info::OwnedAttributeEntry::flags_string`]
//! and by the Python `AttributeFlags.to_typeql_annotations` mirror. The
//! migration planner and the diff classifier both need to reason about the
//! individual annotations inside such a string: TypeDB 3.x `redefine` mutates
//! exactly one schema element per query, parameterless annotations cannot be
//! redefined at all, and `@doc`/`@meta` changes are metadata-only. This module
//! splits a flag string back into its annotation tokens.

use std::collections::BTreeMap;

/// One `@name` or `@name(args)` annotation split out of a flag string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationToken {
    /// Annotation name without the `@`, e.g. `key`, `card`, `doc`, `meta`.
    pub name: String,
    /// Raw argument text between the parentheses, e.g. `1..5`, `"x"`,
    /// `"k", "v"`. `None` for parameterless annotations.
    pub args: Option<String>,
}

impl AnnotationToken {
    /// Render the token back to its `@name` / `@name(args)` form.
    pub fn render(&self) -> String {
        match &self.args {
            Some(args) => format!("@{}({})", self.name, args),
            None => format!("@{}", self.name),
        }
    }

    /// Whether this token is a `@doc` or `@meta` annotation.
    pub fn is_doc_meta(&self) -> bool {
        self.name == "doc" || self.name == "meta"
    }

    /// The identity key for change grouping: `@meta` annotations are keyed by
    /// their first string-literal argument (one value per key per subject);
    /// every other annotation is keyed by its name (at most one per subject).
    pub fn identity(&self) -> String {
        if self.name == "meta"
            && let Some(key) = self.meta_key()
        {
            return format!("meta:{key}");
        }
        self.name.clone()
    }

    /// The meta key of a `@meta` token (unescaped), or `None` for other tokens.
    pub fn meta_key(&self) -> Option<String> {
        if self.name != "meta" {
            return None;
        }
        first_string_literal(self.args.as_deref()?)
    }

    /// Build a `@doc("...")` token from an unescaped doc value.
    pub fn doc(value: &str) -> Self {
        AnnotationToken {
            name: "doc".to_string(),
            args: Some(escaped_string_literal(value)),
        }
    }

    /// Build a `@meta("key", "value")` token from unescaped key and value.
    pub fn meta(key: &str, value: &str) -> Self {
        AnnotationToken {
            name: "meta".to_string(),
            args: Some(format!(
                "{}, {}",
                escaped_string_literal(key),
                escaped_string_literal(value)
            )),
        }
    }
}

/// Split a flag string into its annotation tokens.
///
/// The tokenizer is escape-aware: parentheses inside double-quoted string
/// literals (with `\"` escapes) do not terminate an argument list. Text
/// outside `@...` tokens is ignored, so the input may carry leading/trailing
/// whitespace or separators.
pub fn split_annotation_tokens(flags: &str) -> Vec<AnnotationToken> {
    let bytes = flags.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        i += 1;
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == name_start {
            continue;
        }
        let name = flags[name_start..i].to_string();
        let mut args = None;
        if i < bytes.len() && bytes[i] == b'(' {
            i += 1;
            let args_start = i;
            let mut depth: u32 = 1;
            let mut in_string = false;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if in_string => {
                        // Skip the escaped byte inside a string literal.
                        i += 1;
                    }
                    b'"' => in_string = !in_string,
                    b'(' if !in_string => depth += 1,
                    b')' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            args = Some(flags[args_start..i.min(bytes.len())].to_string());
            if i < bytes.len() {
                // Consume the closing parenthesis.
                i += 1;
            }
        }
        tokens.push(AnnotationToken { name, args });
    }
    tokens
}

/// Render the non-`@doc`/`@meta` tokens of a flag string, space-separated.
///
/// Used by the diff classifier to decide whether a flag change is
/// metadata-only (same constraints, different doc/meta).
pub fn constraint_part(flags: &str) -> String {
    split_annotation_tokens(flags)
        .into_iter()
        .filter(|token| !token.is_doc_meta())
        .map(|token| token.render())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the TypeQL text contains a `@doc(`/`@meta(` schema annotation
/// (TypeDB 3.12+) outside of string literals.
///
/// Used by the version gate to refuse sending annotation-bearing schema DDL
/// to pre-3.12 servers, which would reject it with a syntax error.
pub fn typeql_uses_schema_annotations(typeql: &str) -> bool {
    let bytes = typeql.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => {
                // Skip the escaped byte inside a string literal.
                i += 1;
            }
            b'"' => in_string = !in_string,
            b'@' if !in_string => {
                let rest = &typeql[i..];
                if rest.starts_with("@doc(") || rest.starts_with("@meta(") {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Render a TypeQL string literal, escaping backslash, quote, `\n`, `\t`,
/// and `\r` — the escape set mirrored by the server's schema export.
pub fn escaped_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Extract the first double-quoted string literal from an argument list,
/// unescaping `\\`, `\"`, `\n`, `\t`, and `\r` — the escape set produced by
/// the flag-string emitters.
fn first_string_literal(args: &str) -> Option<String> {
    let start = args.find('"')?;
    let mut out = String::new();
    let mut chars = args[start + 1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// Group two token lists into (added, removed, changed) by token identity.
///
/// `changed` pairs the old and new token for identities present on both sides
/// with different rendered forms. Identities are annotation names, except
/// `@meta`, which is keyed per meta key.
pub fn diff_annotation_tokens(
    old: &[AnnotationToken],
    new: &[AnnotationToken],
) -> AnnotationTokenDiff {
    let old_map: BTreeMap<String, &AnnotationToken> =
        old.iter().map(|t| (t.identity(), t)).collect();
    let new_map: BTreeMap<String, &AnnotationToken> =
        new.iter().map(|t| (t.identity(), t)).collect();

    let mut diff = AnnotationTokenDiff::default();
    for (identity, new_token) in &new_map {
        match old_map.get(identity) {
            None => diff.added.push((*new_token).clone()),
            Some(old_token) => {
                if old_token.render() != new_token.render() {
                    diff.changed
                        .push(((*old_token).clone(), (*new_token).clone()));
                }
            }
        }
    }
    for (identity, old_token) in &old_map {
        if !new_map.contains_key(identity) {
            diff.removed.push((*old_token).clone());
        }
    }
    diff
}

/// Added/removed/changed annotation tokens between two flag strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotationTokenDiff {
    /// Tokens present only in the new flag string.
    pub added: Vec<AnnotationToken>,
    /// Tokens present only in the old flag string.
    pub removed: Vec<AnnotationToken>,
    /// Tokens present on both sides with different values: `(old, new)`.
    pub changed: Vec<(AnnotationToken, AnnotationToken)>,
}

impl AnnotationTokenDiff {
    /// Whether the diff contains no changes at all.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(name: &str, args: Option<&str>) -> AnnotationToken {
        AnnotationToken {
            name: name.to_string(),
            args: args.map(str::to_string),
        }
    }

    #[test]
    fn splits_parameterless_and_parameterized_tokens() {
        let tokens = split_annotation_tokens("@key @card(1..5) @doc(\"a b\") @meta(\"k\", \"v\")");
        assert_eq!(
            tokens,
            vec![
                token("key", None),
                token("card", Some("1..5")),
                token("doc", Some("\"a b\"")),
                token("meta", Some("\"k\", \"v\"")),
            ]
        );
    }

    #[test]
    fn string_literals_shield_parentheses_and_escapes() {
        let tokens = split_annotation_tokens(r#"@doc("has ) paren and \" quote") @unique"#);
        assert_eq!(
            tokens,
            vec![
                token("doc", Some(r#""has ) paren and \" quote""#)),
                token("unique", None),
            ]
        );
    }

    #[test]
    fn meta_identity_is_keyed_per_meta_key() {
        let tokens = split_annotation_tokens(r#"@meta("a", "1") @meta("b", "2")"#);
        assert_eq!(tokens[0].identity(), "meta:a");
        assert_eq!(tokens[1].identity(), "meta:b");
    }

    #[test]
    fn constraint_part_drops_doc_meta() {
        assert_eq!(constraint_part(r#"@key @doc("x") @meta("k", "v")"#), "@key");
        assert_eq!(constraint_part(r#"@doc("x")"#), "");
    }

    #[test]
    fn diff_groups_added_removed_changed() {
        let old = split_annotation_tokens(r#"@key @card(1..5) @doc("old") @meta("a", "1")"#);
        let new = split_annotation_tokens(r#"@key @card(1..9) @meta("a", "2") @meta("b", "3")"#);
        let diff = diff_annotation_tokens(&old, &new);
        assert_eq!(diff.added, vec![token("meta", Some("\"b\", \"3\""))]);
        assert_eq!(diff.removed, vec![token("doc", Some("\"old\""))]);
        assert_eq!(
            diff.changed,
            vec![
                (token("card", Some("1..5")), token("card", Some("1..9"))),
                (
                    token("meta", Some("\"a\", \"1\"")),
                    token("meta", Some("\"a\", \"2\""))
                ),
            ]
        );
    }

    #[test]
    fn identical_strings_diff_empty() {
        let tokens = split_annotation_tokens(r#"@unique @doc("same")"#);
        assert!(diff_annotation_tokens(&tokens, &tokens).is_empty());
    }

    #[test]
    fn typeql_annotation_scan_ignores_string_literals() {
        assert!(typeql_uses_schema_annotations(
            "define\nentity person @doc(\"docs\");"
        ));
        assert!(typeql_uses_schema_annotations(
            "define\nperson owns name @meta(\"k\", \"v\");"
        ));
        assert!(!typeql_uses_schema_annotations(
            "define\nattribute note, value string @values(\"use @doc(x) here\");"
        ));
        assert!(!typeql_uses_schema_annotations(
            "define\nentity person, owns name @key;"
        ));
    }
}
