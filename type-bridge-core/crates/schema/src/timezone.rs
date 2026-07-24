use chrono::{FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::temporal::{
    CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, TimeZoneDesignator,
};
use type_bridge_contract::value::CanonicalValue;

/// Exact named-timezone database policy used by the TypeDB 3.12.1 schema profile.
///
/// `chrono-tz` 0.9.0 embeds IANA tzdb 2024a, matching TypeDB 3.12.1. A policy
/// change must use a new identifier because it can change resolved UTC instants.
pub const TYPEDB_3_12_1_TIMEZONE_POLICY_ID: &str = "typedb-iana-2024a/v1";

/// Exact temporal scalar domain accepted by the TypeDB 3.12.1 Rust driver.
pub const TYPEDB_3_12_1_TEMPORAL_POLICY_ID: &str = "typedb-3.12.1-temporal/v1";

/// Parse a canonical timezone-aware scalar and resolve named zones under the
/// exact TypeDB 3.12.1 timezone policy.
pub fn parse_provider_datetime_tz(value: &str) -> Result<CanonicalDateTimeTz, Diagnostic> {
    if let Some(body) = value.strip_suffix(']') {
        let (local, name) = body.rsplit_once('[').ok_or_else(|| {
            timezone_diagnostic(
                "invalid_provider_datetime_tz",
                "named timezone values must end with one bracketed IANA zone",
            )
        })?;
        if local.contains(['[', ']']) || name.is_empty() {
            return Err(timezone_diagnostic(
                "invalid_provider_datetime_tz",
                "named timezone values must contain one non-empty bracketed IANA zone",
            ));
        }
        let local = local.parse::<CanonicalDateTime>()?;
        return resolve_named(local, name);
    }
    value.parse::<CanonicalDateTimeTz>()
}

/// Parse exact datetime-tz evidence emitted by the driver bridge.
///
/// V2 evidence preserves both an IANA name and the effective offset as
/// `<local><offset>[<name>]`, which is sufficient to distinguish both sides
/// of a daylight-saving overlap. The upstream drivers' legacy
/// `<local> <name>` display remains accepted for ordinary unambiguous values.
pub fn parse_provider_datetime_tz_evidence(value: &str) -> Result<CanonicalDateTimeTz, Diagnostic> {
    if let Some(body) = value.strip_suffix(']') {
        let (prefix, name) = body.rsplit_once('[').ok_or_else(|| {
            timezone_diagnostic(
                "invalid_provider_datetime_tz_evidence",
                "named datetime-tz evidence must end with one bracketed IANA zone",
            )
        })?;
        if prefix.contains(['[', ']']) || name.is_empty() {
            return Err(timezone_diagnostic(
                "invalid_provider_datetime_tz_evidence",
                "named datetime-tz evidence must contain one non-empty bracketed IANA zone",
            ));
        }
        if let Ok(fixed) = prefix.parse::<CanonicalDateTimeTz>() {
            return resolve_named_evidence(fixed.local(), name, fixed.effective_offset_seconds());
        }
        let local = prefix.parse::<CanonicalDateTime>()?;
        return resolve_named(local, name);
    }
    if let Some((local, name)) = value.rsplit_once(' ')
        && !name.is_empty()
        && !local.contains(' ')
    {
        let local = local.parse::<CanonicalDateTime>().or_else(|_| {
            parse_legacy_driver_local_datetime(local).ok_or_else(|| {
                timezone_diagnostic(
                    "invalid_provider_datetime_tz_evidence",
                    "legacy named datetime-tz evidence is not an upstream driver display",
                )
            })
        })?;
        return resolve_named(local, name);
    }
    value.parse::<CanonicalDateTimeTz>().or_else(|_| {
        parse_legacy_driver_fixed_datetime_tz(value).ok_or_else(|| {
            timezone_diagnostic(
                "invalid_provider_datetime_tz_evidence",
                "datetime-tz evidence is outside the exact or legacy driver grammar",
            )
        })
    })
}

/// Parse the `%FT%T%.9f` local datetime emitted by every released driver band.
///
/// This intentionally lives only on the provider-evidence compatibility path.
/// Canonical V2 documents continue to reject insignificant trailing fractional
/// zeroes rather than accepting and normalizing them.
fn parse_legacy_driver_local_datetime(value: &str) -> Option<CanonicalDateTime> {
    let (whole_seconds, fraction) = value.rsplit_once('.')?;
    if fraction.len() != 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let significant = fraction.trim_end_matches('0');
    let canonical = if significant.is_empty() {
        whole_seconds.to_owned()
    } else {
        format!("{whole_seconds}.{significant}")
    };
    canonical.parse().ok()
}

/// Parse the `%FT%T%.9f%:z` fixed-zone display emitted by released drivers.
fn parse_legacy_driver_fixed_datetime_tz(value: &str) -> Option<CanonicalDateTimeTz> {
    let split = value.len().checked_sub(6)?;
    if !value.is_char_boundary(split) {
        return None;
    }
    let local = parse_legacy_driver_local_datetime(&value[..split])?;
    let offset_seconds = parse_legacy_driver_offset(&value[split..])?;
    let zone = if offset_seconds == 0 {
        TimeZoneDesignator::Utc
    } else {
        TimeZoneDesignator::OffsetSeconds(offset_seconds)
    };
    CanonicalDateTimeTz::new_fixed(local, zone).ok()
}

fn parse_legacy_driver_offset(value: &str) -> Option<i32> {
    let [
        sign @ (b'+' | b'-'),
        hour_tens,
        hour_ones,
        b':',
        minute_tens,
        minute_ones,
    ] = value.as_bytes()
    else {
        return None;
    };
    if ![hour_tens, hour_ones, minute_tens, minute_ones]
        .into_iter()
        .all(u8::is_ascii_digit)
    {
        return None;
    }
    let hours = i32::from(hour_tens - b'0') * 10 + i32::from(hour_ones - b'0');
    let minutes = i32::from(minute_tens - b'0') * 10 + i32::from(minute_ones - b'0');
    if hours > 23 || minutes > 59 {
        return None;
    }
    let magnitude = hours * 3_600 + minutes * 60;
    if magnitude == 0 {
        return (*sign == b'+').then_some(0);
    }
    Some(if *sign == b'-' { -magnitude } else { magnitude })
}

/// Resolve a written timezone designator without selecting an arbitrary side
/// of an IANA daylight-saving gap or overlap.
pub fn resolve_provider_datetime_tz(
    local: CanonicalDateTime,
    zone: TimeZoneDesignator,
) -> Result<CanonicalDateTimeTz, Diagnostic> {
    match zone {
        TimeZoneDesignator::Named(name) => resolve_named(local, &name),
        fixed => CanonicalDateTimeTz::new_fixed(local, fixed),
    }
}

/// Validate a carried canonical datetime-tz against the pinned provider
/// timezone database without performing provider I/O.
pub fn validate_provider_datetime_tz(value: &CanonicalDateTimeTz) -> Result<(), Diagnostic> {
    match value.zone() {
        TimeZoneDesignator::Named(name) => {
            if let Err(error) =
                resolve_named_evidence(value.local(), name, value.effective_offset_seconds())
            {
                if error.code().as_str() == "provider_datetime_tz_evidence_offset_mismatch" {
                    return Err(named_offset_mismatch(value, name));
                }
                return Err(error);
            }
        }
        TimeZoneDesignator::Utc | TimeZoneDesignator::OffsetSeconds(_) => {
            let local = chrono_datetime(value.local())?;
            let offset = FixedOffset::east_opt(value.effective_offset_seconds())
                .ok_or_else(provider_datetime_tz_out_of_range)?;
            if offset.from_local_datetime(&local).single().is_none() {
                return Err(provider_datetime_tz_out_of_range());
            }
        }
    }
    Ok(())
}

/// Validate the exact unsigned component domain used by the TypeDB 3.12.1
/// driver duration type (`u32` months/days and `u64` total nanoseconds).
pub fn validate_provider_duration(value: CanonicalDuration) -> Result<(), Diagnostic> {
    let (negative, months, days, seconds, nanosecond) = value.components();
    let nanos = seconds
        .checked_mul(1_000_000_000)
        .and_then(|nanos| nanos.checked_add(u64::from(nanosecond)));
    if negative || u32::try_from(months).is_err() || u32::try_from(days).is_err() || nanos.is_none()
    {
        return Err(temporal_diagnostic(
            "provider_duration_out_of_range",
            "duration is outside the exact TypeDB 3.12.1 driver scalar domain",
        ));
    }
    Ok(())
}

/// Validate temporal values that may cross the TypeDB 3.12.1 driver API.
pub fn validate_provider_temporal_value(value: &CanonicalValue) -> Result<(), Diagnostic> {
    match value {
        CanonicalValue::DateTimeTz(value) => validate_provider_datetime_tz(value),
        CanonicalValue::Duration(value) => validate_provider_duration(*value),
        CanonicalValue::String(_)
        | CanonicalValue::Long(_)
        | CanonicalValue::Double(_)
        | CanonicalValue::Boolean(_)
        | CanonicalValue::Date(_)
        | CanonicalValue::DateTime(_)
        | CanonicalValue::Decimal(_) => Ok(()),
    }
}

/// Validate temporal values that must be written as TypeQL literals.
///
/// The 3.12.1 grammar admits fixed datetime-tz offsets only to minute
/// resolution. Driver-bound `given` values retain seconds and use the broader
/// validator above; literal-only paths fail closed instead of truncating.
pub fn validate_provider_temporal_literal(value: &CanonicalValue) -> Result<(), Diagnostic> {
    validate_provider_temporal_value(value)?;
    if let CanonicalValue::DateTimeTz(value) = value {
        match value.zone() {
            TimeZoneDesignator::Named(name) => {
                let resolved = resolve_named(value.local(), name)?;
                if resolved.effective_offset_seconds() != value.effective_offset_seconds() {
                    return Err(named_offset_mismatch(value, name));
                }
            }
            TimeZoneDesignator::OffsetSeconds(seconds) if seconds % 60 != 0 => {
                return Err(temporal_diagnostic(
                    "provider_datetime_tz_literal_offset_precision",
                    "TypeDB 3.12.1 TypeQL literals cannot represent fixed offsets with seconds",
                ));
            }
            TimeZoneDesignator::Utc | TimeZoneDesignator::OffsetSeconds(_) => {}
        }
    }
    Ok(())
}

fn named_offset_mismatch(value: &CanonicalDateTimeTz, name: &str) -> Diagnostic {
    timezone_diagnostic(
        "provider_datetime_tz_offset_mismatch",
        "named timezone effective offset does not match the pinned provider resolution",
    )
    .with_detail("zone", name)
    .with_detail(
        "carried_offset_seconds",
        value.effective_offset_seconds().to_string(),
    )
}

fn resolve_named(
    local: CanonicalDateTime,
    authored_name: &str,
) -> Result<CanonicalDateTimeTz, Diagnostic> {
    let timezone = named_timezone(authored_name)?;
    let naive = chrono_datetime(local)?;
    let resolved = match timezone.from_local_datetime(&naive) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(_, _) => {
            return Err(timezone_diagnostic(
                "ambiguous_named_timezone_local_datetime",
                "named timezone local datetime maps to multiple UTC instants",
            )
            .with_detail("zone", authored_name));
        }
        LocalResult::None => {
            return Err(timezone_diagnostic(
                "nonexistent_named_timezone_local_datetime",
                "named timezone local datetime does not map to a UTC instant",
            )
            .with_detail("zone", authored_name));
        }
    };
    CanonicalDateTimeTz::new_named_resolved(
        local,
        authored_name,
        resolved.offset().fix().local_minus_utc(),
    )
}

fn resolve_named_evidence(
    local: CanonicalDateTime,
    authored_name: &str,
    effective_offset_seconds: i32,
) -> Result<CanonicalDateTimeTz, Diagnostic> {
    let timezone = named_timezone(authored_name)?;
    let naive = chrono_datetime(local)?;
    let offset_matches = |value: chrono::DateTime<chrono_tz::Tz>| {
        value.offset().fix().local_minus_utc() == effective_offset_seconds
    };
    let matches = match timezone.from_local_datetime(&naive) {
        LocalResult::Single(value) => offset_matches(value),
        LocalResult::Ambiguous(earlier, later) => offset_matches(earlier) || offset_matches(later),
        LocalResult::None => {
            return Err(timezone_diagnostic(
                "nonexistent_named_timezone_local_datetime",
                "named timezone local datetime does not map to a UTC instant",
            )
            .with_detail("zone", authored_name));
        }
    };
    if !matches {
        return Err(timezone_diagnostic(
            "provider_datetime_tz_evidence_offset_mismatch",
            "provider datetime-tz evidence offset does not match the named timezone",
        )
        .with_detail("zone", authored_name)
        .with_detail(
            "effective_offset_seconds",
            effective_offset_seconds.to_string(),
        ));
    }
    CanonicalDateTimeTz::new_named_resolved(local, authored_name, effective_offset_seconds)
}

fn named_timezone(authored_name: &str) -> Result<chrono_tz::Tz, Diagnostic> {
    chrono_tz::TZ_VARIANTS
        .iter()
        .copied()
        .find(|timezone| timezone.name().eq_ignore_ascii_case(authored_name))
        .ok_or_else(|| {
            timezone_diagnostic(
                "unknown_named_timezone",
                "named timezone is not present in the pinned IANA database",
            )
            .with_detail("zone", authored_name)
        })
}

fn chrono_datetime(value: CanonicalDateTime) -> Result<NaiveDateTime, Diagnostic> {
    let (year, month, day) = value.date().components();
    let (hour, minute, second, nanosecond) = value.time().components();
    let date = NaiveDate::from_ymd_opt(year, u32::from(month), u32::from(day));
    let time = NaiveTime::from_hms_nano_opt(
        u32::from(hour),
        u32::from(minute),
        u32::from(second),
        nanosecond,
    );
    date.zip(time)
        .map(|(date, time)| NaiveDateTime::new(date, time))
        .ok_or_else(provider_datetime_tz_out_of_range)
}

fn provider_datetime_tz_out_of_range() -> Diagnostic {
    timezone_diagnostic(
        "provider_datetime_tz_out_of_range",
        "datetime-tz instant is outside the pinned provider implementation range",
    )
}

fn timezone_diagnostic(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static timezone diagnostic code is valid"),
        message,
    )
    .with_detail("timezone_policy", TYPEDB_3_12_1_TIMEZONE_POLICY_ID)
}

fn temporal_diagnostic(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static temporal diagnostic code is valid"),
        message,
    )
    .with_detail("temporal_policy", TYPEDB_3_12_1_TEMPORAL_POLICY_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(value: &str) -> CanonicalDateTime {
        value.parse().expect("test datetime is canonical")
    }

    #[test]
    fn pinned_policy_resolves_case_insensitively_and_preserves_authored_name() {
        let value = resolve_provider_datetime_tz(
            local("2024-07-01T12:00:00"),
            TimeZoneDesignator::Named("europe/paris".to_owned()),
        )
        .expect("ordinary local datetime resolves");
        assert_eq!(value.effective_offset_seconds(), 7_200);
        assert_eq!(value.to_string(), "2024-07-01T12:00:00[europe/paris]");
    }

    #[test]
    fn pinned_policy_rejects_daylight_saving_gaps_and_overlaps() {
        let gap = resolve_provider_datetime_tz(
            local("2024-03-31T01:30:00"),
            TimeZoneDesignator::Named("Europe/London".to_owned()),
        )
        .expect_err("gap has no UTC instant");
        assert_eq!(
            gap.code().as_str(),
            "nonexistent_named_timezone_local_datetime"
        );

        let overlap = resolve_provider_datetime_tz(
            local("2024-10-27T01:30:00"),
            TimeZoneDesignator::Named("Europe/London".to_owned()),
        )
        .expect_err("overlap has two UTC instants");
        assert_eq!(
            overlap.code().as_str(),
            "ambiguous_named_timezone_local_datetime"
        );
    }

    #[test]
    fn carried_named_offsets_must_match_the_pinned_resolution() {
        let ordinary = CanonicalDateTimeTz::new_named_resolved(
            local("2024-07-01T12:00:00"),
            "Europe/Amsterdam",
            7_200,
        )
        .expect("canonical named value");
        validate_provider_datetime_tz(&ordinary).expect("matching offset");

        let mismatch = CanonicalDateTimeTz::new_named_resolved(
            local("2024-07-01T12:00:00"),
            "Europe/Amsterdam",
            1_172,
        )
        .expect("structurally valid forged value");
        assert_eq!(
            validate_provider_datetime_tz(&mismatch)
                .expect_err("carried offset must be verified")
                .code()
                .as_str(),
            "provider_datetime_tz_offset_mismatch"
        );

        let gap = CanonicalDateTimeTz::new_named_resolved(
            local("2024-03-31T01:30:00"),
            "Europe/London",
            3_600,
        )
        .expect("structurally valid carried value");
        assert_eq!(
            validate_provider_datetime_tz(&gap)
                .expect_err("gap inputs fail closed")
                .code()
                .as_str(),
            "nonexistent_named_timezone_local_datetime"
        );

        let overlap = CanonicalDateTimeTz::new_named_resolved(
            local("2024-10-27T01:30:00"),
            "Europe/London",
            3_600,
        )
        .expect("explicit earlier overlap side");
        validate_provider_datetime_tz(&overlap)
            .expect("given transport retains the selected overlap offset");
        assert_eq!(
            validate_provider_temporal_literal(&CanonicalValue::DateTimeTz(overlap))
                .expect_err("TypeQL named literals cannot spell an overlap side")
                .code()
                .as_str(),
            "ambiguous_named_timezone_local_datetime"
        );
    }

    #[test]
    fn fixed_datetime_tz_instants_must_fit_the_driver_utc_range() {
        for local in ["-262143-01-01T00:00:00", "+262142-12-31T23:59:59.999999999"] {
            let value = CanonicalDateTimeTz::new_fixed(
                local.parse().expect("canonical edge"),
                TimeZoneDesignator::Utc,
            )
            .expect("canonical UTC edge");
            validate_provider_datetime_tz(&value).expect("UTC preserves both provider edges");
        }

        for (local, offset) in [
            ("-262143-01-01T00:00:00", 86_399),
            ("+262142-12-31T23:59:59.999999999", -86_399),
        ] {
            let value = CanonicalDateTimeTz::new_fixed(
                local.parse().expect("canonical edge"),
                TimeZoneDesignator::OffsetSeconds(offset),
            )
            .expect("structurally valid fixed edge");
            assert_eq!(
                validate_provider_datetime_tz(&value)
                    .expect_err("offset pushes the UTC instant outside the driver domain")
                    .code()
                    .as_str(),
                "provider_datetime_tz_out_of_range",
            );
        }

        for (local, offset) in [
            ("-262143-01-01T00:00:00", -86_399),
            ("+262142-12-31T23:59:59.999999999", 86_399),
        ] {
            let value = CanonicalDateTimeTz::new_fixed(
                local.parse().expect("canonical edge"),
                TimeZoneDesignator::OffsetSeconds(offset),
            )
            .expect("structurally valid fixed edge");
            validate_provider_datetime_tz(&value)
                .expect("offset shifts the UTC instant inward from the driver edge");
        }
    }

    #[test]
    fn exact_named_evidence_preserves_overlap_side_and_legacy_display_is_supported() {
        let earlier =
            parse_provider_datetime_tz_evidence("2024-10-27T01:30:00+01:00[Europe/London]")
                .expect("explicit earlier overlap offset");
        let later = parse_provider_datetime_tz_evidence("2024-10-27T01:30:00Z[Europe/London]")
            .expect("explicit later overlap offset");
        assert_eq!(earlier.effective_offset_seconds(), 3_600);
        assert_eq!(later.effective_offset_seconds(), 0);
        assert!(earlier.semantic_utc_nanoseconds() < later.semantic_utc_nanoseconds());

        let legacy =
            parse_provider_datetime_tz_evidence("2024-07-01T12:00:00.000000000 Europe/Amsterdam")
                .expect("upstream driver display");
        assert_eq!(legacy.effective_offset_seconds(), 7_200);
        assert_eq!(
            legacy.zone(),
            &TimeZoneDesignator::Named("Europe/Amsterdam".into())
        );

        assert_eq!(
            parse_provider_datetime_tz_evidence("2024-10-27T01:30:00+02:00[Europe/London]")
                .expect_err("forged evidence offset")
                .code()
                .as_str(),
            "provider_datetime_tz_evidence_offset_mismatch"
        );
    }

    #[test]
    fn legacy_driver_display_compatibility_is_narrow_and_wire_parsing_stays_strict() {
        let named =
            parse_provider_datetime_tz_evidence("2024-07-01T12:00:00.125000000 Europe/Amsterdam")
                .expect("released named-zone driver display");
        assert_eq!(named.local().to_string(), "2024-07-01T12:00:00.125");
        assert_eq!(
            named.zone(),
            &TimeZoneDesignator::Named("Europe/Amsterdam".to_owned())
        );

        let fixed = parse_provider_datetime_tz_evidence("+10000-01-02T03:04:05.500000000-05:30")
            .expect("released fixed-zone driver display");
        assert_eq!(fixed.local().to_string(), "+10000-01-02T03:04:05.5");
        assert_eq!(fixed.zone(), &TimeZoneDesignator::OffsetSeconds(-19_800));

        let fixed_utc = parse_provider_datetime_tz_evidence("2024-01-02T03:04:05.000000000+00:00")
            .expect("released zero-offset fixed-zone driver display");
        assert_eq!(fixed_utc.zone(), &TimeZoneDesignator::Utc);

        assert!(
            "2024-01-02T03:04:05.000000000Z"
                .parse::<CanonicalDateTimeTz>()
                .is_err(),
            "the canonical V2 parser must not inherit legacy display normalization"
        );
        for invalid in [
            "2024-01-02T03:04:05.00000000 Europe/Amsterdam",
            "2024-01-02T03:04:05.0000000000 Europe/Amsterdam",
            "2024-01-02T03:04:05.00000000x Europe/Amsterdam",
            "2024-01-02T03:04:05.000000000  Europe/Amsterdam",
            "2024-01-02T03:04:05.000000000-00:00",
            "2024-01-02T03:04:05.000000000+24:00",
            "2024-01-02T03:04:05.000000000+01:60",
        ] {
            assert!(
                parse_provider_datetime_tz_evidence(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn duration_validation_matches_the_driver_component_domain() {
        for valid in [
            CanonicalDuration::new(false, u64::from(u32::MAX), u64::from(u32::MAX), 0, 0).unwrap(),
            CanonicalDuration::new(
                false,
                0,
                0,
                u64::MAX / 1_000_000_000,
                u32::try_from(u64::MAX % 1_000_000_000).unwrap(),
            )
            .unwrap(),
        ] {
            validate_provider_duration(valid).expect("within exact driver domain");
        }
        for invalid in [
            CanonicalDuration::new(true, 0, 1, 0, 0).unwrap(),
            CanonicalDuration::new(false, u64::from(u32::MAX) + 1, 0, 0, 0).unwrap(),
            CanonicalDuration::new(false, 0, u64::from(u32::MAX) + 1, 0, 0).unwrap(),
            CanonicalDuration::new(false, 0, 0, u64::MAX / 1_000_000_000 + 1, 0).unwrap(),
        ] {
            assert_eq!(
                validate_provider_duration(invalid)
                    .expect_err("outside exact driver domain")
                    .code()
                    .as_str(),
                "provider_duration_out_of_range"
            );
        }
    }
}
