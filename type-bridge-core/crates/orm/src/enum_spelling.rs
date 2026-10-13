//! The canonical spelling of every closed enum a binding accepts as text.
//!
//! Each enum owns its parser (`str::parse`, `ACCEPTED`, `spelling`); this
//! module only names the enums so a binding can check a spelling by kind,
//! as the generated Python owner lookup and the enum-spelling parity corpus
//! do.

use type_bridge_contract::enum_spelling::UnknownEnumValue;
use type_bridge_contract::projected_record::ProjectedRecordKind;
use type_bridge_contract::projection::{ManagerLookup, OwnerLookup};
use type_bridge_contract::value::ValueTypeTag;

use crate::_attribute::ValueType;
use crate::match_request::{
    ComparisonOp, MatchMode, MissingOrder, Reduction, RowCardinality, SortDirection,
};
use crate::projected_manager_filter::ProjectedManagerComparison;
use crate::session::{GivenColumnType, TxType};

/// One closed enum a binding accepts as text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EnumKind {
    /// [`ComparisonOp`].
    ComparisonOperator,
    /// [`SortDirection`].
    SortDirection,
    /// [`MissingOrder`].
    MissingOrder,
    /// [`RowCardinality`].
    RowCardinality,
    /// [`Reduction`].
    Reducer,
    /// [`TxType`].
    TransactionType,
    /// [`ValueType`] (TypeQL spelling, `datetime-tz`).
    ValueType,
    /// [`ValueTypeTag`] (contract spelling, `datetime_tz`).
    ScalarDomain,
    /// [`ProjectedRecordKind`].
    ProjectedRecordKind,
    /// [`ManagerLookup`].
    ManagerLookup,
    /// [`ProjectedManagerComparison`].
    ManagerComparison,
    /// [`OwnerLookup`].
    OwnerLookup,
    /// [`MatchMode`].
    MatchMode,
    /// [`GivenColumnType`].
    GivenColumnType,
}

type_bridge_contract::enum_spellings!(EnumKind, "enum_kind", canonical {
    "comparison_operator" => ComparisonOperator,
    "sort_direction" => SortDirection,
    "missing_order" => MissingOrder,
    "row_cardinality" => RowCardinality,
    "reducer" => Reducer,
    "transaction_type" => TransactionType,
    "value_type" => ValueType,
    "scalar_domain" => ScalarDomain,
    "projected_record_kind" => ProjectedRecordKind,
    "manager_lookup" => ManagerLookup,
    "manager_comparison" => ManagerComparison,
    "owner_lookup" => OwnerLookup,
    "match_mode" => MatchMode,
    "given_column_type" => GivenColumnType,
});

impl EnumKind {
    /// Parse `value` with this kind's parser and return its canonical
    /// spelling (an alias such as `exact` becomes `eq`).
    pub fn canonical(self, value: &str) -> Result<&'static str, UnknownEnumValue> {
        match self {
            Self::ComparisonOperator => value.parse().map(ComparisonOp::spelling),
            Self::SortDirection => value.parse().map(SortDirection::spelling),
            Self::MissingOrder => value.parse().map(MissingOrder::spelling),
            Self::RowCardinality => value.parse().map(RowCardinality::spelling),
            Self::Reducer => value.parse().map(Reduction::spelling),
            Self::TransactionType => value.parse().map(TxType::spelling),
            Self::ValueType => value.parse().map(ValueType::spelling),
            Self::ScalarDomain => value.parse().map(ValueTypeTag::spelling),
            Self::ProjectedRecordKind => value.parse().map(ProjectedRecordKind::spelling),
            Self::ManagerLookup => value.parse().map(ManagerLookup::spelling),
            Self::ManagerComparison => value.parse().map(ProjectedManagerComparison::spelling),
            Self::OwnerLookup => value.parse().map(OwnerLookup::spelling),
            Self::MatchMode => value.parse().map(MatchMode::spelling),
            Self::GivenColumnType => value.parse().map(GivenColumnType::spelling),
        }
    }

    /// Every spelling this kind's parser accepts.
    #[must_use]
    pub const fn accepted(self) -> &'static [&'static str] {
        match self {
            Self::ComparisonOperator => ComparisonOp::ACCEPTED,
            Self::SortDirection => SortDirection::ACCEPTED,
            Self::MissingOrder => MissingOrder::ACCEPTED,
            Self::RowCardinality => RowCardinality::ACCEPTED,
            Self::Reducer => Reduction::ACCEPTED,
            Self::TransactionType => TxType::ACCEPTED,
            Self::ValueType => ValueType::ACCEPTED,
            Self::ScalarDomain => ValueTypeTag::ACCEPTED,
            Self::ProjectedRecordKind => ProjectedRecordKind::ACCEPTED,
            Self::ManagerLookup => ManagerLookup::ACCEPTED,
            Self::ManagerComparison => ProjectedManagerComparison::ACCEPTED,
            Self::OwnerLookup => OwnerLookup::ACCEPTED,
            Self::MatchMode => MatchMode::ACCEPTED,
            Self::GivenColumnType => GivenColumnType::ACCEPTED,
        }
    }
}

/// Check `value` against the enum named by `kind` and return its canonical
/// spelling. An unknown `kind` is itself an unknown `enum_kind` spelling.
pub fn canonical_enum_spelling(kind: &str, value: &str) -> Result<&'static str, UnknownEnumValue> {
    kind.parse::<EnumKind>()?.canonical(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_names_its_parser_and_round_trips_every_spelling() {
        for kind_spelling in EnumKind::ACCEPTED {
            let kind: EnumKind = kind_spelling.parse().unwrap();
            let error = kind.canonical("\u{0}").unwrap_err();
            assert_eq!(error.kind(), *kind_spelling);
            assert_eq!(error.accepted(), kind.accepted());
            for spelling in kind.accepted() {
                let canonical = kind.canonical(spelling).unwrap();
                assert_eq!(kind.canonical(canonical), Ok(canonical));
            }
        }
    }

    #[test]
    fn canonical_spellings_match_the_serde_wire_names() {
        fn serde_name<T: serde::Serialize>(value: T) -> String {
            serde_json::to_value(value)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        }
        for spelling in ComparisonOp::ACCEPTED {
            let value: ComparisonOp = spelling.parse().unwrap();
            assert_eq!(serde_name(value), *spelling);
        }
        for spelling in Reduction::ACCEPTED {
            let value: Reduction = spelling.parse().unwrap();
            assert_eq!(serde_name(value), *spelling);
        }
        for spelling in RowCardinality::ACCEPTED {
            let value: RowCardinality = spelling.parse().unwrap();
            assert_eq!(serde_name(value), *spelling);
        }
        for spelling in ValueType::ACCEPTED {
            let value: ValueType = spelling.parse().unwrap();
            assert_eq!(serde_name(value), *spelling);
        }
        for spelling in ValueTypeTag::ACCEPTED {
            let value: ValueTypeTag = spelling.parse().unwrap();
            assert_eq!(serde_name(value), value.spelling());
        }
        assert_eq!(serde_name(SortDirection::Descending), "descending");
        assert_eq!(serde_name(MissingOrder::First), "first");
        for spelling in MatchMode::ACCEPTED {
            let value: MatchMode = spelling.parse().unwrap();
            assert_eq!(serde_name(value), *spelling);
        }
    }

    #[test]
    fn aliases_are_the_ones_the_core_documents() {
        assert_eq!(canonical_enum_spelling("manager_lookup", "exact"), Ok("eq"));
        assert_eq!(
            canonical_enum_spelling("manager_lookup", "like"),
            Ok("regex")
        );
        assert_eq!(canonical_enum_spelling("owner_lookup", "exact"), Ok("eq"));
        assert!(canonical_enum_spelling("owner_lookup", "like").is_err());
        assert_eq!(
            canonical_enum_spelling("transaction_type", " READ "),
            Ok("read")
        );
        assert!(canonical_enum_spelling("transaction_type", "read only").is_err());
        assert!(canonical_enum_spelling("value_type", "datetime_tz").is_err());
        assert_eq!(
            canonical_enum_spelling("scalar_domain", "datetime-tz"),
            Ok("datetime_tz")
        );
        assert_eq!(
            canonical_enum_spelling("given_column_type", "long"),
            Ok("integer")
        );
        let error = canonical_enum_spelling("sort", "ascending").unwrap_err();
        assert_eq!(error.kind(), "enum_kind");
    }
}
