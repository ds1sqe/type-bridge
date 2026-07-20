use chrono::{LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::temporal::{CanonicalDateTime, CanonicalDateTimeTz, TimeZoneDesignator};

/// Exact named-timezone database policy used by the TypeDB 3.12.1 schema profile.
///
/// `chrono-tz` 0.9.0 embeds IANA tzdb 2024a, matching TypeDB 3.12.1. A policy
/// change must use a new identifier because it can change resolved UTC instants.
pub const TYPEDB_3_12_1_TIMEZONE_POLICY_ID: &str = "typedb-iana-2024a/v1";

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

fn resolve_named(
    local: CanonicalDateTime,
    authored_name: &str,
) -> Result<CanonicalDateTimeTz, Diagnostic> {
    let timezone = chrono_tz::TZ_VARIANTS
        .iter()
        .copied()
        .find(|timezone| timezone.name().eq_ignore_ascii_case(authored_name))
        .ok_or_else(|| {
            timezone_diagnostic(
                "unknown_named_timezone",
                "named timezone is not present in the pinned IANA database",
            )
            .with_detail("zone", authored_name)
        })?;
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
        .ok_or_else(|| {
            timezone_diagnostic(
                "provider_datetime_tz_out_of_range",
                "datetime is outside the pinned provider timezone implementation range",
            )
        })
}

fn timezone_diagnostic(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static timezone diagnostic code is valid"),
        message,
    )
    .with_detail("timezone_policy", TYPEDB_3_12_1_TIMEZONE_POLICY_ID)
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
}
