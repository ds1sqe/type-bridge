//! One parser per closed enum: canonical spellings, aliases, and the single
//! `unknown_enum_value` diagnostic every binding raises for anything else.
//!
//! Bindings never match enum strings themselves. They call `str::parse` on
//! the core enum and lower an
//! [`UnknownEnumValue`](crate::enum_spelling::UnknownEnumValue) into the
//! structured SDK diagnostic, so an unknown spelling has the same category, code, and
//! details in every language.

use std::fmt;

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkExecutionDiagnostic, SdkQueryDiagnosticIdentity,
};

/// Stable diagnostic code for a spelling that no core enum parser accepts.
pub const UNKNOWN_ENUM_VALUE_CODE: &str = "unknown_enum_value";

const UNKNOWN_ENUM_VALUE_MESSAGE: &str = "The value is not an accepted spelling of its enum";

/// A spelling rejected by one core enum parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownEnumValue {
    kind: &'static str,
    value: String,
    accepted: &'static [&'static str],
}

impl UnknownEnumValue {
    /// Record one rejected spelling of the enum `kind`.
    #[must_use]
    pub fn new(kind: &'static str, value: &str, accepted: &'static [&'static str]) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            accepted,
        }
    }

    /// The stable enum kind name, such as `sort_direction`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    /// The rejected text, exactly as given.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Every spelling the parser accepts, canonical spellings first.
    #[must_use]
    pub const fn accepted(&self) -> &'static [&'static str] {
        self.accepted
    }

    /// The rejected text when it is a bounded identifier (no whitespace,
    /// control characters, `/`, `\`, or `@`, at most 512 bytes); both
    /// diagnostic lowerings report `value` only then, so caller text cannot
    /// smuggle free text into a diagnostic.
    fn reportable_value(&self) -> Option<SdkQueryDiagnosticIdentity> {
        SdkQueryDiagnosticIdentity::new(self.value.as_str()).ok()
    }
}

impl fmt::Display for UnknownEnumValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown {} {:?}; expected one of {}",
            self.kind,
            self.value,
            self.accepted.join(", ")
        )
    }
}

impl std::error::Error for UnknownEnumValue {}

impl From<UnknownEnumValue> for SdkExecutionDiagnostic {
    /// Details: `kind` and `accepted` always; `value` only when the rejected
    /// text is a bounded identifier.
    fn from(error: UnknownEnumValue) -> Self {
        let identity = |text: &str| {
            SdkQueryDiagnosticIdentity::new(text).expect("enum spellings are bounded identifiers")
        };
        let name = |text: &str| SdkDiagnosticName::new(text).expect("detail key is canonical");
        let diagnostic = Self::invalid_input(
            SdkDiagnosticCode::new(UNKNOWN_ENUM_VALUE_CODE).expect("code is canonical"),
            SdkDiagnosticMessage::new(UNKNOWN_ENUM_VALUE_MESSAGE).expect("message is canonical"),
        )
        .try_with_detail(
            name("kind"),
            SdkDiagnosticDetailValue::QueryIdentity(identity(error.kind)),
        )
        .and_then(|diagnostic| {
            diagnostic.try_with_detail(
                name("accepted"),
                SdkDiagnosticDetailValue::QueryIdentityList(
                    error
                        .accepted
                        .iter()
                        .map(|spelling| identity(spelling))
                        .collect(),
                ),
            )
        })
        .expect("two distinct details fit the SDK detail ceiling");
        match error.reportable_value() {
            Some(value) => diagnostic
                .try_with_detail(
                    name("value"),
                    SdkDiagnosticDetailValue::QueryIdentity(value),
                )
                .expect("three distinct details fit the SDK detail ceiling"),
            None => diagnostic,
        }
    }
}

impl From<UnknownEnumValue> for Diagnostic {
    /// The contract-diagnostic form used by the query-plan builder, whose
    /// authored-input failures are all `invalid_contract`: same code,
    /// message, and detail keys as the SDK form, as text details.
    fn from(error: UnknownEnumValue) -> Self {
        let diagnostic = Self::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new(UNKNOWN_ENUM_VALUE_CODE).expect("code is canonical"),
            UNKNOWN_ENUM_VALUE_MESSAGE,
        )
        .with_detail("kind", error.kind)
        .with_detail(
            "accepted",
            error
                .accepted
                .iter()
                .map(|spelling| (*spelling).to_owned())
                .collect::<Vec<_>>(),
        );
        match error.reportable_value() {
            Some(value) => diagnostic.with_detail("value", value.as_str()),
            None => diagnostic,
        }
    }
}

/// Fold a spelling for a parser declared `folded`: surrounding whitespace
/// trimmed and ASCII letters lowercased. Other parsers match exactly.
#[doc(hidden)]
#[must_use]
pub fn normalize_spelling(value: &str, folded: bool) -> std::borrow::Cow<'_, str> {
    if folded {
        std::borrow::Cow::Owned(value.trim().to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

/// Give a closed enum its one parser.
///
/// Generates `KIND`, `ACCEPTED` (canonical spellings, then aliases), a
/// `const fn spelling(self)` returning each variant's canonical spelling,
/// and `FromStr` with [`UnknownEnumValue`] as the error. The `canonical`
/// block must name every variant exactly once; the exhaustive `spelling`
/// match enforces it at compile time. Writing `folded,` before `canonical`
/// makes the parser trim surrounding whitespace and ignore ASCII case
/// (canonical spellings and aliases must then be lowercase); a rejected
/// spelling is still reported exactly as given.
#[doc(hidden)]
#[macro_export]
macro_rules! enum_spellings {
    (
        @impl $ty:ty, $kind:literal, $folded:literal,
        canonical { $($spelling:literal => $variant:ident),+ }
        $(, aliases { $($alias:literal => $alias_variant:ident),+ })?
    ) => {
        impl $ty {
            /// Stable enum kind name used in `unknown_enum_value` details.
            pub const KIND: &'static str = $kind;
            /// Every accepted spelling: canonical spellings, then aliases.
            pub const ACCEPTED: &'static [&'static str] =
                &[$($spelling,)+ $($($alias,)+)?];

            /// The canonical spelling of this value.
            #[must_use]
            pub const fn spelling(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling,)+
                }
            }
        }

        impl ::core::str::FromStr for $ty {
            type Err = $crate::enum_spelling::UnknownEnumValue;

            fn from_str(value: &str) -> ::core::result::Result<Self, Self::Err> {
                match $crate::enum_spelling::normalize_spelling(value, $folded).as_ref() {
                    $($spelling => Ok(Self::$variant),)+
                    $($($alias => Ok(Self::$alias_variant),)+)?
                    _ => Err($crate::enum_spelling::UnknownEnumValue::new(
                        $kind,
                        value,
                        Self::ACCEPTED,
                    )),
                }
            }
        }
    };
    (
        $ty:ty, $kind:literal, folded,
        canonical { $($spelling:literal => $variant:ident),+ $(,)? }
        $(, aliases { $($alias:literal => $alias_variant:ident),+ $(,)? })? $(,)?
    ) => {
        $crate::enum_spellings!(
            @impl $ty, $kind, true,
            canonical { $($spelling => $variant),+ }
            $(, aliases { $($alias => $alias_variant),+ })?
        );
    };
    (
        $ty:ty, $kind:literal,
        canonical { $($spelling:literal => $variant:ident),+ $(,)? }
        $(, aliases { $($alias:literal => $alias_variant:ident),+ $(,)? })? $(,)?
    ) => {
        $crate::enum_spellings!(
            @impl $ty, $kind, false,
            canonical { $($spelling => $variant),+ }
            $(, aliases { $($alias => $alias_variant),+ })?
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk_diagnostic::SdkDiagnosticCategory;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Sample {
        One,
        Two,
    }

    enum_spellings!(Sample, "sample", canonical { "one" => One, "two" => Two }, aliases { "uno" => One });

    #[test]
    fn parser_accepts_canonical_spellings_and_aliases_only() {
        assert_eq!(
            (Sample::KIND, Sample::ACCEPTED),
            ("sample", &["one", "two", "uno"][..])
        );
        assert_eq!("one".parse(), Ok(Sample::One));
        assert_eq!("uno".parse(), Ok(Sample::One));
        assert_eq!(Sample::One.spelling(), "one");
        let error = "One".parse::<Sample>().unwrap_err();
        assert_eq!(" one ".parse::<Sample>().unwrap_err().value(), " one ");
        assert_eq!(
            (error.kind(), error.value(), error.accepted()),
            ("sample", "One", Sample::ACCEPTED)
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Folded {
        One,
    }

    enum_spellings!(Folded, "folded", folded, canonical { "one" => One });

    #[test]
    fn folded_parser_trims_and_ignores_ascii_case() {
        assert_eq!("ONE".parse(), Ok(Folded::One));
        assert_eq!(" One\t".parse(), Ok(Folded::One));
        assert_eq!((Folded::KIND, Folded::One.spelling()), ("folded", "one"));
        let error = "o ne".parse::<Folded>().unwrap_err();
        assert_eq!((error.value(), error.accepted()), ("o ne", &["one"][..]));
    }

    #[test]
    fn unknown_spelling_is_one_structured_diagnostic() {
        let diagnostic = SdkExecutionDiagnostic::from("three".parse::<Sample>().unwrap_err());
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
        assert_eq!(diagnostic.code().as_str(), UNKNOWN_ENUM_VALUE_CODE);
        let details = diagnostic
            .details()
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>();
        let identity = |text: &str| SdkQueryDiagnosticIdentity::new(text).unwrap();
        assert_eq!(
            details,
            [
                (
                    "accepted",
                    SdkDiagnosticDetailValue::QueryIdentityList(vec![
                        identity("one"),
                        identity("two"),
                        identity("uno"),
                    ])
                ),
                (
                    "kind",
                    SdkDiagnosticDetailValue::QueryIdentity(identity("sample"))
                ),
                (
                    "value",
                    SdkDiagnosticDetailValue::QueryIdentity(identity("three"))
                ),
            ]
        );
    }

    #[test]
    fn free_text_value_is_left_out_of_the_diagnostic() {
        for value in ["", "two words", "a/b", "line\nbreak", &"x".repeat(513)] {
            let diagnostic = SdkExecutionDiagnostic::from(value.parse::<Sample>().unwrap_err());
            let keys = diagnostic
                .details()
                .keys()
                .map(SdkDiagnosticName::as_str)
                .collect::<Vec<_>>();
            assert_eq!(keys, ["accepted", "kind"], "{value:?}");
        }
    }

    #[test]
    fn contract_form_has_the_same_code_message_and_detail_keys() {
        use crate::diagnostic::DiagnosticDetailValue;

        let diagnostic = Diagnostic::from("three".parse::<Sample>().unwrap_err());
        assert_eq!(diagnostic.category(), DiagnosticCategory::InvalidContract);
        assert_eq!(diagnostic.code().as_str(), UNKNOWN_ENUM_VALUE_CODE);
        assert_eq!(diagnostic.message(), UNKNOWN_ENUM_VALUE_MESSAGE);
        let details = diagnostic
            .details()
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            details,
            [
                (
                    "accepted",
                    DiagnosticDetailValue::TextList(vec!["one".into(), "two".into(), "uno".into()])
                ),
                ("kind", DiagnosticDetailValue::Text("sample".into())),
                ("value", DiagnosticDetailValue::Text("three".into())),
            ]
        );
        let free_text = Diagnostic::from("two words".parse::<Sample>().unwrap_err());
        let keys = free_text
            .details()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(keys, ["accepted", "kind"]);
    }
}
