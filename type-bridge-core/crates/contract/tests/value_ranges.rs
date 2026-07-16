use std::cmp::Ordering;

use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::schema::{
    CanonicalValueRange, CanonicalValueRangeViolation, CanonicalValueSet,
    CanonicalValueSetViolation,
};
use type_bridge_contract::temporal::{
    CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, TimeZoneDesignator,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};

#[test]
fn ranges_use_domain_semantics_not_representation_order() {
    let decimal = |value| CanonicalValue::Decimal(DecimalValue::new(value).unwrap());
    assert!(CanonicalValueRange::new(Some(decimal("2")), Some(decimal("10"))).is_ok());
    assert_eq!(
        CanonicalValueRange::new(Some(decimal("-2")), Some(decimal("-10")))
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_range_annotation_bounds",
    );

    let negative_zero = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
    let positive_zero = CanonicalValue::Double(CanonicalDouble::new(0.0).unwrap());
    assert_eq!(
        CanonicalValueRange::new_detailed(Some(negative_zero), Some(positive_zero)),
        Err(CanonicalValueRangeViolation::InvalidBounds { ordering: Ordering::Equal }),
    );

    assert!(CanonicalValueRange::new(
        Some(CanonicalValue::Boolean(false)),
        Some(CanonicalValue::Boolean(true)),
    )
    .is_ok());
    assert!(CanonicalValueRange::new(
        Some(CanonicalValue::String(CanonicalString::new("alpha").unwrap())),
        Some(CanonicalValue::String(CanonicalString::new("beta").unwrap())),
    )
    .is_ok());
}

#[test]
fn temporal_ranges_are_chronological_and_timezone_independent() {
    let utc = CanonicalValue::DateTimeTz(
        CanonicalDateTimeTz::new_fixed(
            "2024-01-01T12:00:00".parse::<CanonicalDateTime>().unwrap(),
            TimeZoneDesignator::Utc,
        )
        .unwrap(),
    );
    let same_instant = CanonicalValue::DateTimeTz(
        CanonicalDateTimeTz::new_fixed(
            "2024-01-01T13:00:00".parse::<CanonicalDateTime>().unwrap(),
            TimeZoneDesignator::OffsetSeconds(3600),
        )
        .unwrap(),
    );
    assert_eq!(
        CanonicalValueRange::new_detailed(Some(utc), Some(same_instant)),
        Err(CanonicalValueRangeViolation::InvalidBounds { ordering: Ordering::Equal }),
    );

    assert!(CanonicalValueRange::new(
        Some(CanonicalValue::Date("-0001-12-31".parse().unwrap())),
        Some(CanonicalValue::Date("0000-01-01".parse().unwrap())),
    )
    .is_ok());
}

#[test]
fn range_violations_preserve_compatibility_codes() {
    assert_eq!(
        CanonicalValueRange::new_detailed(
            Some(CanonicalValue::Long(1)),
            Some(CanonicalValue::Decimal(DecimalValue::new("2").unwrap())),
        ),
        Err(CanonicalValueRangeViolation::MixedDomain {
            lower: ValueTypeTag::Long,
            upper: ValueTypeTag::Decimal,
        }),
    );
    assert_eq!(
        CanonicalValueRange::new(Some(CanonicalValue::Long(1)), Some(CanonicalValue::Double(CanonicalDouble::new(2.0).unwrap())))
            .unwrap_err()
            .code()
            .as_str(),
        "mixed_range_annotation_domain",
    );

    let duration = CanonicalValue::Duration(CanonicalDuration::new(false, 0, 1, 0, 0).unwrap());
    let error = CanonicalValueRange::new(Some(duration), None).unwrap_err();
    assert_eq!(error.category(), DiagnosticCategory::InvalidContract);
    assert_eq!(error.code().as_str(), "unsupported_range_annotation_domain");
}

#[test]
fn value_set_reports_raw_member_positions_and_limit() {
    assert_eq!(
        CanonicalValueSet::new_detailed([CanonicalValue::Long(7), CanonicalValue::Long(7)]),
        Err(CanonicalValueSetViolation::Duplicate { first_index: 0, duplicate_index: 1 }),
    );
    assert_eq!(
        CanonicalValueSet::new_detailed([CanonicalValue::Long(7), CanonicalValue::Boolean(true)]),
        Err(CanonicalValueSetViolation::MixedDomain {
            expected: ValueTypeTag::Long,
            actual: ValueTypeTag::Boolean,
            member_index: 1,
        }),
    );

    let error = CanonicalValueSet::new((0_i64..=65_536).map(CanonicalValue::Long)).unwrap_err();
    assert_eq!(error.category(), DiagnosticCategory::ResourceLimit);
    assert_eq!(error.code().as_str(), "values_annotation_member_limit_exceeded");
}
