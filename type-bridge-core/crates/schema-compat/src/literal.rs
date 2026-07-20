use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDuration, CanonicalTime, TimeZoneDesignator,
};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use typeql::value::{
    DateFragment, DurationDate, DurationLiteral, DurationTime, Literal, Sign, SignedDecimalLiteral,
    SignedDoubleLiteral, SignedIntegerLiteral, StringLiteral, TimeFragment, TimeZone, ValueLiteral,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiteralConversionError {
    code: &'static str,
    message: String,
}

impl LiteralConversionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) fn canonical_literal(
    literal: &Literal,
) -> Result<CanonicalValue, LiteralConversionError> {
    match &literal.inner {
        ValueLiteral::Boolean(value) => match value.value.as_str() {
            "true" => Ok(CanonicalValue::Boolean(true)),
            "false" => Ok(CanonicalValue::Boolean(false)),
            _ => Err(invalid(
                "boolean",
                "boolean literal must be `true` or `false`",
            )),
        },
        ValueLiteral::Integer(value) => canonical_integer(value),
        ValueLiteral::Double(value) => canonical_double(value),
        ValueLiteral::Decimal(value) => canonical_decimal(value),
        ValueLiteral::String(value) => canonical_string(value),
        ValueLiteral::Date(value) => canonical_date(&value.date).map(CanonicalValue::Date),
        ValueLiteral::DateTime(value) => {
            canonical_datetime(&value.date, &value.time).map(CanonicalValue::DateTime)
        }
        ValueLiteral::DateTimeTz(value) => {
            let local = canonical_datetime(&value.date, &value.time)?;
            let zone = canonical_timezone(&value.timezone)?;
            type_bridge_schema::resolve_provider_datetime_tz(local, zone)
                .map(CanonicalValue::DateTimeTz)
                .map_err(|error| invalid("datetime_tz", error.to_string()))
        }
        ValueLiteral::Duration(value) => canonical_duration(value).map(CanonicalValue::Duration),
        ValueLiteral::Struct(_) => Err(LiteralConversionError::new(
            "unsupported_typeql_struct_literal",
            "struct literals are not canonical scalar annotation values",
        )),
    }
}

fn canonical_integer(
    value: &SignedIntegerLiteral,
) -> Result<CanonicalValue, LiteralConversionError> {
    let text = signed_text(value.sign, &value.integral);
    text.parse::<i64>().map(CanonicalValue::Long).map_err(|_| {
        invalid(
            "integer",
            "integer literal is outside the signed 64-bit domain",
        )
    })
}

fn canonical_double(value: &SignedDoubleLiteral) -> Result<CanonicalValue, LiteralConversionError> {
    let text = signed_text(value.sign, &value.double);
    let parsed = text
        .parse::<f64>()
        .map_err(|_| invalid("double", "double literal is not valid binary64 text"))?;
    CanonicalDouble::new(parsed)
        .map(CanonicalValue::Double)
        .map_err(|error| invalid("double", error.to_string()))
}

fn canonical_decimal(
    value: &SignedDecimalLiteral,
) -> Result<CanonicalValue, LiteralConversionError> {
    let mut text = signed_text(value.sign, &value.decimal);
    text.push_str("dec");
    DecimalValue::new(&text)
        .map(CanonicalValue::Decimal)
        .map_err(|error| invalid("decimal", error.to_string()))
}

fn canonical_string(value: &StringLiteral) -> Result<CanonicalValue, LiteralConversionError> {
    validate_quoted_string(value, "string")?;
    let decoded = value
        .unescape()
        .map_err(|error| invalid("string", error.to_string()))?;
    CanonicalString::new(decoded)
        .map(CanonicalValue::String)
        .map_err(|error| invalid("string", error.to_string()))
}

pub(crate) fn validate_quoted_string(
    value: &StringLiteral,
    domain: &'static str,
) -> Result<(), LiteralConversionError> {
    let bytes = value.value.as_bytes();
    if bytes.len() < 2
        || bytes.first() != bytes.last()
        || !matches!(bytes.first(), Some(b'\'' | b'"'))
    {
        return Err(invalid(
            domain,
            format!("{domain} literal must have matching single or double quotes"),
        ));
    }
    Ok(())
}

fn canonical_date(value: &DateFragment) -> Result<CanonicalDate, LiteralConversionError> {
    let year = parse_date_year(&value.year)?;
    let month = parse_unsigned(&value.month, "date", "month")?
        .try_into()
        .map_err(|_| invalid("date", "date month is outside the u8 domain"))?;
    let day = parse_unsigned(&value.day, "date", "day")?
        .try_into()
        .map_err(|_| invalid("date", "date day is outside the u8 domain"))?;
    CanonicalDate::new(year, month, day).map_err(|error| invalid("date", error.to_string()))
}

fn parse_date_year(value: &str) -> Result<i32, LiteralConversionError> {
    let (signed, digits) = match value.as_bytes().first() {
        Some(b'+' | b'-') => (true, &value[1..]),
        _ => (false, value),
    };
    if digits.is_empty()
        || !digits.as_bytes().iter().all(u8::is_ascii_digit)
        || (!signed && digits.len() != 4)
    {
        return Err(invalid(
            "date",
            "date year must be four unsigned digits or a signed decimal year",
        ));
    }
    value
        .parse::<i32>()
        .map_err(|_| invalid("date", "date year is outside the signed 32-bit domain"))
}

fn canonical_time(value: &TimeFragment) -> Result<CanonicalTime, LiteralConversionError> {
    let hour = parse_unsigned(&value.hour, "datetime", "hour")?
        .try_into()
        .map_err(|_| invalid("datetime", "hour is outside the u8 domain"))?;
    let minute = parse_unsigned(&value.minute, "datetime", "minute")?
        .try_into()
        .map_err(|_| invalid("datetime", "minute is outside the u8 domain"))?;
    let second = value
        .second
        .as_deref()
        .map(|second| parse_unsigned(second, "datetime", "second"))
        .transpose()?
        .unwrap_or(0)
        .try_into()
        .map_err(|_| invalid("datetime", "second is outside the u8 domain"))?;
    let nanosecond = match (&value.second, &value.second_fraction) {
        (None, Some(_)) => {
            return Err(invalid(
                "datetime",
                "fractional seconds require an explicit second component",
            ));
        }
        (_, None) => 0,
        (Some(_), Some(fraction)) => parse_nanoseconds(fraction)?,
    };
    CanonicalTime::new(hour, minute, second, nanosecond)
        .map_err(|error| invalid("datetime", error.to_string()))
}

fn canonical_datetime(
    date: &DateFragment,
    time: &TimeFragment,
) -> Result<CanonicalDateTime, LiteralConversionError> {
    Ok(CanonicalDateTime::new(
        canonical_date(date)?,
        canonical_time(time)?,
    ))
}

fn canonical_timezone(value: &TimeZone) -> Result<TimeZoneDesignator, LiteralConversionError> {
    match value {
        TimeZone::IANA(name) => {
            if name.is_empty() || name.len() > 255 {
                return Err(invalid(
                    "datetime_tz",
                    "named timezone is empty or too long",
                ));
            }
            Ok(TimeZoneDesignator::Named(name.clone()))
        }
        TimeZone::ISO(value) if value == "Z" => Ok(TimeZoneDesignator::Utc),
        TimeZone::ISO(value) => {
            let (sign, body) = match value.as_bytes().first() {
                Some(b'+') => (1_i32, &value[1..]),
                Some(b'-') => (-1_i32, &value[1..]),
                _ => {
                    return Err(invalid(
                        "datetime_tz",
                        "ISO timezone must be Z or begin with a sign",
                    ));
                }
            };
            if !body.is_ascii() {
                return Err(invalid(
                    "datetime_tz",
                    "ISO timezone offsets must contain ASCII digits and separators",
                ));
            }
            let (hours, minutes) = match body.len() {
                2 => (&body[..2], "0"),
                4 => (&body[..2], &body[2..]),
                5 if &body[2..3] == ":" => (&body[..2], &body[3..]),
                _ => {
                    return Err(invalid(
                        "datetime_tz",
                        "ISO timezone must use HH, HHMM, or HH:MM",
                    ));
                }
            };
            let hours = parse_unsigned(hours, "datetime_tz", "timezone hour")?;
            let minutes = parse_unsigned(minutes, "datetime_tz", "timezone minute")?;
            if hours > 23 || minutes > 59 {
                return Err(invalid(
                    "datetime_tz",
                    "timezone offset must not exceed 23:59",
                ));
            }
            let seconds = i32::try_from(hours * 3600 + minutes * 60)
                .map_err(|_| invalid("datetime_tz", "timezone offset overflow"))?
                * sign;
            if seconds == 0 {
                Ok(TimeZoneDesignator::Utc)
            } else {
                Ok(TimeZoneDesignator::OffsetSeconds(seconds))
            }
        }
    }
}

fn canonical_duration(
    value: &DurationLiteral,
) -> Result<CanonicalDuration, LiteralConversionError> {
    let (months, days, seconds, nanosecond) = match value {
        DurationLiteral::Weeks(weeks) => {
            let weeks = parse_unsigned(&weeks.value, "duration", "weeks")?;
            let days = weeks
                .checked_mul(7)
                .ok_or_else(|| invalid("duration", "duration week conversion overflow"))?;
            (0, days, 0, 0)
        }
        DurationLiteral::DateAndTime(date, time) => {
            if !duration_date_has_component(date)
                && time
                    .as_ref()
                    .is_none_or(|time| !duration_time_has_component(time))
            {
                return Err(invalid(
                    "duration",
                    "duration must contain at least one explicit component",
                ));
            }
            if time
                .as_ref()
                .is_some_and(|time| !duration_time_has_component(time))
            {
                return Err(invalid(
                    "duration",
                    "duration time section must contain an explicit component",
                ));
            }
            let (months, days) = duration_date(date)?;
            let (seconds, nanosecond) = time
                .as_ref()
                .map(duration_time)
                .transpose()?
                .unwrap_or((0, 0));
            (months, days, seconds, nanosecond)
        }
        DurationLiteral::Time(time) => {
            if !duration_time_has_component(time) {
                return Err(invalid(
                    "duration",
                    "duration must contain at least one explicit component",
                ));
            }
            let (seconds, nanosecond) = duration_time(time)?;
            (0, 0, seconds, nanosecond)
        }
    };
    CanonicalDuration::new(false, months, days, seconds, nanosecond)
        .map_err(|error| invalid("duration", error.to_string()))
}

fn duration_date_has_component(value: &DurationDate) -> bool {
    value.years.is_some() || value.months.is_some() || value.days.is_some()
}

fn duration_time_has_component(value: &DurationTime) -> bool {
    value.hours.is_some() || value.minutes.is_some() || value.seconds.is_some()
}

#[cfg(test)]
mod defensive_tests {
    use typeql::value::{IntegerLiteral, ValueLiteral};

    use super::*;

    #[test]
    fn extended_signed_years_and_year_zero_are_canonical() {
        for year in ["-262143", "0000", "+262142"] {
            canonical_date(&DateFragment {
                year: year.to_owned(),
                month: "01".to_owned(),
                day: "02".to_owned(),
            })
            .expect("provider-supported year must canonicalize");
        }
        assert_eq!(
            canonical_date(&DateFragment {
                year: "999".to_owned(),
                month: "01".to_owned(),
                day: "02".to_owned(),
            })
            .expect_err("unsigned years require four digits")
            .code(),
            "invalid_typeql_date"
        );
    }

    #[test]
    fn forged_empty_duration_is_rejected_but_explicit_zero_is_valid() {
        let empty = Literal {
            span: None,
            inner: ValueLiteral::Duration(DurationLiteral::DateAndTime(
                DurationDate {
                    years: None,
                    months: None,
                    days: None,
                },
                None,
            )),
        };
        assert_eq!(
            canonical_literal(&empty)
                .expect_err("empty duration AST must fail closed")
                .code(),
            "invalid_typeql_duration"
        );

        let explicit_zero = Literal {
            span: None,
            inner: ValueLiteral::Duration(DurationLiteral::DateAndTime(
                DurationDate {
                    years: None,
                    months: None,
                    days: Some(IntegerLiteral {
                        value: "0".to_owned(),
                    }),
                },
                None,
            )),
        };
        canonical_literal(&explicit_zero).expect("an explicit zero component is meaningful");
    }
}

fn duration_date(value: &DurationDate) -> Result<(u64, u64), LiteralConversionError> {
    let years = optional_unsigned(
        value.years.as_ref().map(|value| value.value.as_str()),
        "years",
    )?;
    let months = optional_unsigned(
        value.months.as_ref().map(|value| value.value.as_str()),
        "months",
    )?;
    let days = optional_unsigned(
        value.days.as_ref().map(|value| value.value.as_str()),
        "days",
    )?;
    let months = years
        .checked_mul(12)
        .and_then(|years| years.checked_add(months))
        .ok_or_else(|| invalid("duration", "duration month conversion overflow"))?;
    Ok((months, days))
}

fn duration_time(value: &DurationTime) -> Result<(u64, u32), LiteralConversionError> {
    let hours = optional_unsigned(
        value.hours.as_ref().map(|value| value.value.as_str()),
        "hours",
    )?;
    let minutes = optional_unsigned(
        value.minutes.as_ref().map(|value| value.value.as_str()),
        "minutes",
    )?;
    let (literal_seconds, nanosecond) = value
        .seconds
        .as_ref()
        .map(|value| decimal_seconds(&value.value))
        .transpose()?
        .unwrap_or((0, 0));
    let seconds = hours
        .checked_mul(3600)
        .and_then(|hours| {
            minutes
                .checked_mul(60)
                .and_then(|minutes| hours.checked_add(minutes))
        })
        .and_then(|seconds| seconds.checked_add(literal_seconds))
        .ok_or_else(|| invalid("duration", "duration time conversion overflow"))?;
    Ok((seconds, nanosecond))
}

fn decimal_seconds(value: &str) -> Result<(u64, u32), LiteralConversionError> {
    let (mantissa, exponent_text) = split_exponent(value)?;
    let mut digits = String::with_capacity(mantissa.len());
    let mut fraction_digits = 0_i64;
    let mut seen_decimal = false;
    for character in mantissa.chars() {
        match character {
            '0'..='9' => {
                digits.push(character);
                if seen_decimal {
                    fraction_digits = fraction_digits
                        .checked_add(1)
                        .ok_or_else(|| invalid("duration", "duration fraction length overflow"))?;
                }
            }
            '.' if !seen_decimal => seen_decimal = true,
            _ => return Err(invalid("duration", "duration seconds are not decimal text")),
        }
    }
    if digits.is_empty() || mantissa.ends_with('.') {
        return Err(invalid("duration", "duration seconds are not decimal text"));
    }
    if digits.bytes().all(|digit| digit == b'0') {
        return Ok((0, 0));
    }
    let exponent = exponent_text
        .map(|value| value.parse::<i64>())
        .transpose()
        .map_err(|_| {
            invalid(
                "duration",
                "duration seconds exponent is outside the supported domain",
            )
        })?
        .unwrap_or(0);
    let leading = digits.bytes().take_while(|digit| *digit == b'0').count();
    digits.drain(..leading);
    let trailing = digits
        .bytes()
        .rev()
        .take_while(|digit| *digit == b'0')
        .count();
    if trailing != 0 {
        digits.truncate(digits.len() - trailing);
    }
    let scale = exponent
        .checked_sub(fraction_digits)
        .and_then(|scale| scale.checked_add(trailing as i64))
        .ok_or_else(|| invalid("duration", "duration seconds scale overflow"))?;
    if scale >= 0 {
        let whole = parse_digit_u64(&digits, "duration seconds")?;
        let power = u32::try_from(scale)
            .ok()
            .and_then(|scale| 10_u64.checked_pow(scale))
            .ok_or_else(|| invalid("duration", "duration seconds overflow"))?;
        return whole
            .checked_mul(power)
            .map(|whole| (whole, 0))
            .ok_or_else(|| invalid("duration", "duration seconds overflow"));
    }
    let fractional_places = scale
        .checked_neg()
        .and_then(|places| u32::try_from(places).ok())
        .ok_or_else(|| invalid("duration", "duration fractional scale overflow"))?;
    if fractional_places > 9 {
        return Err(invalid(
            "duration",
            "duration seconds are not exactly representable as nanoseconds",
        ));
    }
    let split = (digits.len() as i64)
        .checked_add(scale)
        .ok_or_else(|| invalid("duration", "duration seconds scale overflow"))?;
    let (whole, fraction) = if split <= 0 {
        let fraction = parse_digit_u64(&digits, "duration fraction")?;
        let leading_places = u32::try_from(-split)
            .map_err(|_| invalid("duration", "duration fractional scale overflow"))?;
        let total_places = leading_places
            .checked_add(digits.len() as u32)
            .ok_or_else(|| invalid("duration", "duration fractional scale overflow"))?;
        let nanos = fraction
            .checked_mul(
                10_u64
                    .checked_pow(9 - total_places)
                    .ok_or_else(|| invalid("duration", "duration fractional scale overflow"))?,
            )
            .ok_or_else(|| invalid("duration", "duration nanosecond overflow"))?;
        (0, nanos)
    } else {
        let split = usize::try_from(split)
            .map_err(|_| invalid("duration", "duration seconds scale overflow"))?;
        let whole = parse_digit_u64(&digits[..split], "duration seconds")?;
        let fraction_digits = &digits[split..];
        let fraction = parse_digit_u64(fraction_digits, "duration fraction")?;
        let nanos = fraction
            .checked_mul(
                10_u64
                    .checked_pow(9 - fraction_digits.len() as u32)
                    .ok_or_else(|| invalid("duration", "duration fraction overflow"))?,
            )
            .ok_or_else(|| invalid("duration", "duration nanosecond overflow"))?;
        (whole, nanos)
    };
    let nanosecond =
        u32::try_from(fraction).map_err(|_| invalid("duration", "duration nanosecond overflow"))?;
    Ok((whole, nanosecond))
}

fn split_exponent(value: &str) -> Result<(&str, Option<&str>), LiteralConversionError> {
    let mut indices = value.match_indices(['e', 'E']);
    let first = indices.next();
    if indices.next().is_some() {
        return Err(invalid(
            "duration",
            "duration seconds contain multiple exponents",
        ));
    }
    match first {
        Some((index, _)) if index != 0 && index + 1 < value.len() => {
            Ok((&value[..index], Some(&value[index + 1..])))
        }
        Some(_) => Err(invalid(
            "duration",
            "duration seconds exponent is incomplete",
        )),
        None => Ok((value, None)),
    }
}

fn parse_nanoseconds(value: &str) -> Result<u32, LiteralConversionError> {
    if value.is_empty() || value.len() > 9 || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid(
            "datetime",
            "fractional seconds require one through nine decimal digits",
        ));
    }
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid("datetime", "fractional seconds are invalid"))?;
    parsed
        .checked_mul(10_u32.pow(9 - value.len() as u32))
        .ok_or_else(|| invalid("datetime", "fractional seconds overflow"))
}

fn optional_unsigned(value: Option<&str>, component: &str) -> Result<u64, LiteralConversionError> {
    value
        .map(|value| parse_unsigned(value, "duration", component))
        .transpose()
        .map(|value| value.unwrap_or(0))
}

fn parse_unsigned(
    value: &str,
    domain: &'static str,
    component: &str,
) -> Result<u64, LiteralConversionError> {
    if value.is_empty() || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid(
            domain,
            format!("{component} must contain decimal digits"),
        ));
    }
    value.parse::<u64>().map_err(|_| {
        invalid(
            domain,
            format!("{component} is outside the unsigned 64-bit domain"),
        )
    })
}

fn parse_digit_u64(value: &str, component: &str) -> Result<u64, LiteralConversionError> {
    if value.is_empty() {
        return Ok(0);
    }
    value
        .bytes()
        .try_fold(0_u64, |accumulator, digit| {
            accumulator
                .checked_mul(10)
                .and_then(|accumulator| accumulator.checked_add(u64::from(digit - b'0')))
        })
        .ok_or_else(|| invalid("duration", format!("{component} overflow")))
}

fn signed_text(sign: Option<Sign>, value: &str) -> String {
    let mut text = String::with_capacity(value.len() + usize::from(sign.is_some()));
    match sign {
        Some(Sign::Plus) => text.push('+'),
        Some(Sign::Minus) => text.push('-'),
        None => {}
    }
    text.push_str(value);
    text
}

fn invalid(domain: &'static str, message: impl Into<String>) -> LiteralConversionError {
    let code = match domain {
        "boolean" => "invalid_typeql_boolean",
        "integer" => "invalid_typeql_integer",
        "double" => "invalid_typeql_double",
        "decimal" => "invalid_typeql_decimal",
        "string" => "invalid_typeql_string",
        "date" => "invalid_typeql_date",
        "datetime" => "invalid_typeql_datetime",
        "datetime_tz" => "invalid_typeql_datetime_tz",
        "duration" => "invalid_typeql_duration",
        _ => "invalid_typeql_literal",
    };
    LiteralConversionError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use typeql::value::{
        DateTimeTZLiteral, DurationTime, Literal, NumericLiteral, TimeZone, ValueLiteral,
    };

    fn parsed(value: &str) -> Result<CanonicalValue, LiteralConversionError> {
        let inner = typeql::parse_value(value).expect("valid TypeQL literal syntax");
        canonical_literal(&Literal { span: None, inner })
    }

    #[test]
    fn numeric_domains_preserve_exact_contract_values() {
        assert_eq!(
            parsed("-9223372036854775808").unwrap(),
            CanonicalValue::Long(i64::MIN)
        );
        let CanonicalValue::Double(double) = parsed("-0.0").unwrap() else {
            panic!("expected double")
        };
        assert_eq!(double.bits(), 0x8000_0000_0000_0000);
        let CanonicalValue::Decimal(decimal) = parsed("+001.2300dec").unwrap() else {
            panic!("expected decimal")
        };
        assert_eq!(decimal.as_str(), "1.23");
        assert!(parsed("1.8e308").is_err());
    }

    #[test]
    fn temporal_domains_validate_and_normalize_components() {
        assert_eq!(
            parsed("2024-02-29").unwrap().to_string_for_test(),
            "2024-02-29"
        );
        assert!(parsed("2023-02-29").is_err());
        let timezone = ValueLiteral::DateTimeTz(DateTimeTZLiteral {
            date: DateFragment {
                year: "2024".to_owned(),
                month: "01".to_owned(),
                day: "02".to_owned(),
            },
            time: TimeFragment {
                hour: "03".to_owned(),
                minute: "04".to_owned(),
                second: None,
                second_fraction: None,
            },
            timezone: TimeZone::ISO("+09".to_owned()),
        });
        let value = canonical_literal(&Literal {
            span: None,
            inner: timezone,
        })
        .unwrap();
        let CanonicalValue::DateTimeTz(value) = value else {
            panic!("expected timezone datetime")
        };
        assert_eq!(value.to_string(), "2024-01-02T03:04:00+09:00");
        assert!(canonical_timezone(&TimeZone::ISO("+é".to_owned())).is_err());

        let named = ValueLiteral::DateTimeTz(DateTimeTZLiteral {
            date: DateFragment {
                year: "2024".to_owned(),
                month: "07".to_owned(),
                day: "01".to_owned(),
            },
            time: TimeFragment {
                hour: "12".to_owned(),
                minute: "00".to_owned(),
                second: None,
                second_fraction: None,
            },
            timezone: TimeZone::IANA("europe/paris".to_owned()),
        });
        let value = canonical_literal(&Literal {
            span: None,
            inner: named,
        })
        .unwrap();
        let CanonicalValue::DateTimeTz(value) = value else {
            panic!("expected named timezone datetime")
        };
        assert_eq!(value.effective_offset_seconds(), 7_200);
        assert_eq!(value.to_string(), "2024-07-01T12:00:00[europe/paris]");
    }

    #[test]
    fn duration_seconds_are_integer_normalized_to_nanoseconds() {
        let duration = |seconds: &str| {
            canonical_literal(&Literal {
                span: None,
                inner: ValueLiteral::Duration(DurationLiteral::Time(DurationTime {
                    hours: None,
                    minutes: None,
                    seconds: Some(NumericLiteral {
                        value: seconds.to_owned(),
                    }),
                })),
            })
        };
        let CanonicalValue::Duration(value) = duration("1.2e3").unwrap() else {
            panic!("expected duration")
        };
        assert_eq!(value.components(), (false, 0, 0, 1200, 0));
        let CanonicalValue::Duration(value) = duration("1.0e-9").unwrap() else {
            panic!("expected duration")
        };
        assert_eq!(value.components(), (false, 0, 0, 0, 1));
        assert!(duration("1.2e-9").is_err());
    }

    trait CanonicalValueTestDisplay {
        fn to_string_for_test(&self) -> String;
    }

    impl CanonicalValueTestDisplay for CanonicalValue {
        fn to_string_for_test(&self) -> String {
            match self {
                CanonicalValue::Date(value) => value.to_string(),
                _ => panic!("test helper only supports dates"),
            }
        }
    }
}
