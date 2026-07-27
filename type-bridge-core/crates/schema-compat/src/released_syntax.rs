//! Length-preserving normalization for the frozen released TypeQL grammar.
//!
//! This module is deliberately private to the compatibility front-end.  It
//! never changes the strict TypeQL importer: it first requires the frozen V1
//! parser to accept the source, then records the owner selected by that
//! parser's outer declaration loops before rewriting separators in place and
//! folding repeated `define` blocks into one strict query. Every rewrite is
//! byte-length preserving, so strict-parser spans still index the caller's
//! original source.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use type_bridge_core_lib::parser::{
    SourceRegionKind, blank_source_extents, parse_typeql, scan_source_regions,
};
use typeql::common::identifier::Identifier;
use typeql::type_::Label;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleasedAnnotationTarget {
    Type,
    Value,
    Capability,
}

#[derive(Debug)]
pub(crate) struct ReleasedSyntax {
    original: String,
    source: String,
    annotation_targets: BTreeMap<usize, ReleasedAnnotationTarget>,
    restored_labels: BTreeMap<usize, String>,
}

impl ReleasedSyntax {
    /// Normalize only input accepted by the frozen released parser.
    pub(crate) fn accepted(source: &str) -> Option<Self> {
        Self::accepted_with_size_policy(source, crate::TypeqlSourceSizePolicy::Defensive)
    }

    pub(crate) fn accepted_with_size_policy(
        source: &str,
        size_policy: crate::TypeqlSourceSizePolicy,
    ) -> Option<Self> {
        if !size_policy.allows(source.len()) {
            return None;
        }
        parse_typeql(source).ok()?;
        Some(Self::normalize(source))
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn original_source(&self) -> &str {
        &self.original
    }

    pub(crate) fn annotation_target(&self, byte_start: usize) -> Option<ReleasedAnnotationTarget> {
        self.annotation_targets.get(&byte_start).copied()
    }

    /// Restore a frozen-parser label that had to be replaced in-place to make
    /// an otherwise ambiguous standalone `plays` statement strict-parseable.
    pub(crate) fn restore_label(&self, label: &mut Label) {
        let Some(span) = label.span else {
            return;
        };
        let Some(original) = self.restored_labels.get(&span.begin_offset) else {
            return;
        };
        label.ident = Identifier::new(label.ident.span, original.clone());
    }

    fn normalize(source: &str) -> Self {
        let mut statement = Vec::new();
        let mut normalization = StatementNormalization::default();
        let mut code_separator_slots = BTreeSet::new();
        let mut code_newline_slots = BTreeSet::new();
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        let mut braces = 0_u32;

        for (range, kind) in scan_source_regions(source) {
            if kind != SourceRegionKind::Code {
                continue;
            }
            let bytes = source.as_bytes();
            let mut cursor = range.start;
            while cursor < range.end {
                let byte = bytes[cursor];
                if parentheses > 0 || brackets > 0 || braces > 0 {
                    match byte {
                        b'(' => parentheses += 1,
                        b')' => parentheses = parentheses.saturating_sub(1),
                        b'[' => brackets += 1,
                        b']' => brackets = brackets.saturating_sub(1),
                        b'{' => braces += 1,
                        b'}' => braces = braces.saturating_sub(1),
                        _ => {}
                    }
                    cursor += source[cursor..].chars().next().map_or(1, char::len_utf8);
                    continue;
                }

                match byte {
                    b'(' => parentheses = 1,
                    b'[' => brackets = 1,
                    b'{' => braces = 1,
                    b';' => {
                        process_statement(
                            &statement,
                            &mut normalization,
                            &code_separator_slots,
                            &code_newline_slots,
                        );
                        statement.clear();
                    }
                    b',' => statement.push(Token::Comma { start: cursor }),
                    b':' => statement.push(Token::Colon { start: cursor }),
                    b'@' => {
                        let end = scan_identifier_end(source, cursor + 1, range.end);
                        if end > cursor + 1 {
                            statement.push(Token::Annotation {
                                start: cursor,
                                end,
                                name: &source[cursor + 1..end],
                            });
                            cursor = end;
                            continue;
                        }
                        statement.push(Token::Other { end: cursor + 1 });
                    }
                    _ if is_identifier_start(byte) => {
                        let end = scan_identifier_end(source, cursor + 1, range.end);
                        let word = &source[cursor..end];
                        statement.push(Token::Word {
                            start: cursor,
                            end,
                            word,
                        });
                        cursor = end;
                        continue;
                    }
                    _ if byte.is_ascii_whitespace() => {
                        if matches!(byte, b' ' | b'\t' | b'\r') {
                            code_separator_slots.insert(cursor);
                        } else if byte == b'\n' {
                            code_newline_slots.insert(cursor);
                        }
                    }
                    _ => statement.push(Token::Other {
                        end: cursor + source[cursor..].chars().next().map_or(1, char::len_utf8),
                    }),
                }
                cursor += source[cursor..].chars().next().map_or(1, char::len_utf8);
            }
        }
        process_statement(
            &statement,
            &mut normalization,
            &code_separator_slots,
            &code_newline_slots,
        );

        let StatementNormalization {
            annotation_targets,
            commas_to_blank,
            separators_to_commas,
            separators_to_newlines,
            definitions_to_blank,
            labels_to_rewrite,
            ..
        } = normalization;

        let mut extents = commas_to_blank
            .into_iter()
            .map(|position| position..position + 1)
            .collect::<Vec<_>>();
        extents.extend(definitions_to_blank);
        extents.sort_by_key(|extent| extent.start);
        let blanked = blank_source_extents(source, &extents);
        let mut bytes = blanked.into_bytes();
        for separator in separators_to_commas {
            bytes[separator] = b',';
        }
        for separator in separators_to_newlines {
            bytes[separator] = b'\n';
        }
        let restored_labels = labels_to_rewrite
            .into_iter()
            .map(|(start, end)| {
                let original = source[start..end].to_owned();
                bytes[start] = b'x';
                bytes[start + 1..end].fill(b'_');
                (start, original)
            })
            .collect();
        Self {
            original: source.to_owned(),
            source: String::from_utf8(bytes).expect("normalization preserves source UTF-8"),
            annotation_targets,
            restored_labels,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Token<'a> {
    Word {
        start: usize,
        end: usize,
        word: &'a str,
    },
    Annotation {
        start: usize,
        end: usize,
        name: &'a str,
    },
    Comma {
        start: usize,
    },
    Colon {
        start: usize,
    },
    Other {
        end: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeclarationKind {
    Attribute,
    Entity,
    Relation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Clause {
    None,
    Sub,
    Value,
    Owns,
    Plays,
    Relates,
}

#[derive(Default)]
struct StatementNormalization {
    saw_define: bool,
    definitions_to_blank: Vec<Range<usize>>,
    labels_to_rewrite: BTreeMap<usize, usize>,
    annotation_targets: BTreeMap<usize, ReleasedAnnotationTarget>,
    commas_to_blank: BTreeSet<usize>,
    separators_to_commas: BTreeSet<usize>,
    separators_to_newlines: BTreeSet<usize>,
}

fn process_statement(
    tokens: &[Token<'_>],
    normalization: &mut StatementNormalization,
    code_separator_slots: &BTreeSet<usize>,
    code_newline_slots: &BTreeSet<usize>,
) {
    let leading_define_is_marker = leading_define_is_marker(tokens, normalization.saw_define);
    if leading_define_is_marker && let Some((start, end)) = leading_word(tokens, "define") {
        if normalization.saw_define {
            normalization.definitions_to_blank.push(start..end);
        } else {
            normalization.saw_define = true;
        }
    }

    if let Some(player) = standalone_plays_player(tokens, leading_define_is_marker) {
        // The frozen generator accepts `person plays relation:role ...;` as a
        // standalone reopening. The strict parser represents it as a kindless
        // type declaration, but every annotation on that spelling belongs to
        // its single plays capability. Retain that ownership explicitly so
        // the compatibility importer does not silently move or discard
        // @card/@doc/@meta while inferring the player's kind later.
        if let Token::Word { start, end, word } = tokens[player]
            && matches!(word, "attribute" | "define" | "entity" | "relation")
        {
            normalization.labels_to_rewrite.insert(start, end);
        }
        for token in tokens {
            if let Token::Annotation { start, .. } = token {
                normalization
                    .annotation_targets
                    .insert(*start, ReleasedAnnotationTarget::Capability);
            }
        }
        return;
    }

    let Some((kind, declaration_keyword)) = declaration_kind(tokens) else {
        return;
    };

    let label = tokens[declaration_keyword + 1..]
        .iter()
        .position(|token| matches!(token, Token::Word { .. }))
        .map(|offset| declaration_keyword + 1 + offset);
    let Some(label) = label else {
        return;
    };
    let statement_start = token_start(&tokens[0]);
    let mut clause = Clause::None;
    let mut required_words = 0_u8;
    let mut preceding_comma = None;
    let mut previous_end = token_end(&tokens[label]);
    for token in &tokens[label + 1..] {
        match token {
            Token::Word { start, end, word } => {
                if required_words > 0 {
                    required_words -= 1;
                    previous_end = *end;
                    preceding_comma = None;
                    continue;
                }
                if clause == Clause::Relates && *word == "as" {
                    required_words = 1;
                    previous_end = *end;
                    preceding_comma = None;
                    continue;
                }
                if let Some(next_clause) = clause_keyword(word) {
                    if preceding_comma.is_none() {
                        if let Some(separator) =
                            code_separator_slots.range(previous_end..*start).next_back()
                        {
                            normalization.separators_to_commas.insert(*separator);
                        } else if let Some(newline) =
                            code_newline_slots.range(previous_end..*start).next_back()
                            && let Some(movable) = code_separator_slots
                                .range(statement_start..*newline)
                                .rev()
                                .find(|position| {
                                    !normalization.separators_to_commas.contains(position)
                                        && !normalization.separators_to_newlines.contains(position)
                                })
                        {
                            // A bare LF is the only separator byte available.
                            // Move that line break to an earlier whitespace
                            // slot and use its original byte for the comma. The
                            // importer retains `original` for provenance, so
                            // offsets and reported line/column locations still
                            // describe the caller's source.
                            normalization.separators_to_commas.insert(*newline);
                            normalization.separators_to_newlines.insert(*movable);
                        }
                    }
                    clause = next_clause;
                    required_words = clause_required_words(next_clause);
                }
                previous_end = *end;
                preceding_comma = None;
            }
            Token::Comma { start } => {
                clause = Clause::None;
                required_words = 0;
                preceding_comma = Some(*start);
                previous_end = start + 1;
            }
            Token::Colon { start } => {
                previous_end = start + 1;
                preceding_comma = None;
            }
            Token::Other { end } => {
                previous_end = *end;
                preceding_comma = None;
            }
            Token::Annotation { start, end, name } => {
                let target = annotation_target(kind, clause, name);
                normalization.annotation_targets.insert(*start, target);
                if target != ReleasedAnnotationTarget::Capability {
                    if let Some(comma) = preceding_comma {
                        normalization.commas_to_blank.insert(comma);
                    }
                    clause = Clause::None;
                    required_words = 0;
                }
                previous_end = *end;
                preceding_comma = None;
            }
        }
    }
}

fn leading_define_is_marker(tokens: &[Token<'_>], saw_define: bool) -> bool {
    let Some(Token::Word { word: "define", .. }) = tokens.first() else {
        return false;
    };
    !saw_define || !has_standalone_plays_shape(tokens, 0)
}

fn leading_word(tokens: &[Token<'_>], expected: &str) -> Option<(usize, usize)> {
    match tokens.first() {
        Some(Token::Word { start, end, word }) if *word == expected => Some((*start, *end)),
        _ => None,
    }
}

fn standalone_plays_player(tokens: &[Token<'_>], leading_define_is_marker: bool) -> Option<usize> {
    let player = usize::from(leading_define_is_marker);
    has_standalone_plays_shape(tokens, player).then_some(player)
}

fn has_standalone_plays_shape(tokens: &[Token<'_>], player: usize) -> bool {
    matches!(
        tokens.get(player..player + 5),
        Some([
            Token::Word { .. },
            Token::Word { word: "plays", .. },
            Token::Word { .. },
            Token::Colon { .. },
            Token::Word { .. },
        ])
    )
}

fn declaration_kind(tokens: &[Token<'_>]) -> Option<(DeclarationKind, usize)> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        let Token::Word { word, .. } = token else {
            return None;
        };
        match *word {
            "define" => None,
            "attribute" => Some((DeclarationKind::Attribute, index)),
            "entity" => Some((DeclarationKind::Entity, index)),
            "relation" => Some((DeclarationKind::Relation, index)),
            _ => None,
        }
    })
}

fn clause_keyword(word: &str) -> Option<Clause> {
    match word {
        "sub" => Some(Clause::Sub),
        "value" => Some(Clause::Value),
        "owns" => Some(Clause::Owns),
        "plays" => Some(Clause::Plays),
        "relates" => Some(Clause::Relates),
        _ => None,
    }
}

const fn clause_required_words(clause: Clause) -> u8 {
    match clause {
        Clause::Plays => 2,
        Clause::Sub | Clause::Value | Clause::Owns | Clause::Relates => 1,
        Clause::None => 0,
    }
}

fn token_end(token: &Token<'_>) -> usize {
    match token {
        Token::Word { end, .. } | Token::Annotation { end, .. } | Token::Other { end, .. } => *end,
        Token::Comma { start } | Token::Colon { start } => start + 1,
    }
}

fn token_start(token: &Token<'_>) -> usize {
    match token {
        Token::Word { start, .. }
        | Token::Annotation { start, .. }
        | Token::Comma { start }
        | Token::Colon { start } => *start,
        Token::Other { end } => end.saturating_sub(1),
    }
}

fn annotation_target(
    kind: DeclarationKind,
    clause: Clause,
    name: &str,
) -> ReleasedAnnotationTarget {
    use ReleasedAnnotationTarget::{Capability, Type, Value};

    match kind {
        DeclarationKind::Attribute => match name {
            "regex" | "values" | "range" => Value,
            _ => Type,
        },
        DeclarationKind::Entity => match clause {
            Clause::Owns
                if matches!(
                    name,
                    "key" | "unique" | "cascade" | "subkey" | "distinct" | "card" | "doc" | "meta"
                ) =>
            {
                Capability
            }
            Clause::Plays if matches!(name, "card" | "doc" | "meta") => Capability,
            _ => Type,
        },
        DeclarationKind::Relation => match clause {
            Clause::Owns
                if matches!(
                    name,
                    "key" | "unique" | "cascade" | "subkey" | "distinct" | "card" | "doc" | "meta"
                ) =>
            {
                Capability
            }
            Clause::Plays if matches!(name, "card" | "doc" | "meta") => Capability,
            Clause::Relates
                if matches!(name, "card" | "distinct" | "abstract" | "doc" | "meta") =>
            {
                Capability
            }
            _ => Type,
        },
    }
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

const fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn scan_identifier_end(source: &str, mut cursor: usize, end: usize) -> usize {
    while cursor < end && is_identifier_continue(source.as_bytes()[cursor]) {
        cursor += 1;
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_length_preserving_and_ignores_literal_commas() {
        let source = "define\nattribute email, value string, @regex(\"a,b,@abstract\");\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        assert_eq!(normalized.source().len(), source.len());
        assert!(normalized.source().contains("value string  @regex"));
        assert!(normalized.source().contains("\"a,b,@abstract\""));
    }

    #[test]
    fn bare_lf_separator_moves_without_changing_offsets() {
        let source = "define\nentity person @abstract\nowns name;\nattribute name value string;\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        assert_eq!(normalized.source().len(), source.len());
        assert_eq!(normalized.original_source(), source);
        assert!(normalized.source().contains("person\n@abstract,owns"));
        typeql::parse_queries(normalized.source()).expect("normalized strict TypeQL");
    }

    #[test]
    fn comma_changes_object_annotation_ownership() {
        let source = "define\nattribute name, value string;\nentity person, owns name @doc(\"own\"), @doc(\"type\");\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        let own = source.find("@doc(\"own\")").expect("own doc");
        let type_ = source.find("@doc(\"type\")").expect("type doc");
        assert_eq!(
            normalized.annotation_target(own),
            Some(ReleasedAnnotationTarget::Capability)
        );
        assert_eq!(
            normalized.annotation_target(type_),
            Some(ReleasedAnnotationTarget::Type)
        );
    }

    #[test]
    fn reopened_type_annotations_keep_source_order() {
        let source =
            "define\nentity person @doc(\"first\");\ndefine\nentity person @doc(\"last\");\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        assert_eq!(normalized.source().matches("define").count(), 1);
        for marker in ["@doc(\"first\")", "@doc(\"last\")"] {
            assert_eq!(
                normalized.annotation_target(source.find(marker).expect("annotation")),
                Some(ReleasedAnnotationTarget::Type),
                "fixture: {marker}"
            );
        }
    }

    #[test]
    fn complex_reopened_fixture_uses_released_ownership() {
        let source = "define\n\
            attribute name, value string;\n\
            entity base; entity alternate;\n\
            relation interaction, relates participant @card(0..2) @doc(\"first role\");\n\
            entity person @doc(\"first type\") @meta(\"source\", \"first\"), sub base,\n\
              owns name @card(0..1) @doc(\"first own\"),\n\
              plays interaction:participant @card(0..3) @doc(\"first play\");\n\
            define\n\
            relation interaction, relates participant @card(1..1) @doc(\"ignored role\");\n\
            entity person @doc(\"last type\") @meta(\"source\", \"last\"), sub alternate,\n\
              owns name @key @doc(\"ignored own\"),\n\
              plays interaction:participant @card(1..1) @doc(\"ignored play\");\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        for marker in ["@doc(\"first type\")", "@doc(\"last type\")"] {
            assert_eq!(
                normalized.annotation_target(source.find(marker).expect("annotation")),
                Some(ReleasedAnnotationTarget::Type),
                "fixture: {marker}"
            );
        }
    }

    #[test]
    fn standalone_plays_annotations_belong_to_the_capability() {
        let source = "define\nrelation friendship, relates friend;\n\
                      person plays friendship:friend @card(0..3) @doc(\"edge\") @meta(\"k\", \"v\");\n";
        let normalized = ReleasedSyntax::accepted(source).expect("released source");
        for marker in ["@card", "@doc", "@meta"] {
            let start = source.find(marker).expect("annotation marker");
            assert_eq!(
                normalized.annotation_target(start),
                Some(ReleasedAnnotationTarget::Capability),
                "fixture: {marker}"
            );
        }
    }
}
