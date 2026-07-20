//! Canonical low-dependency temporal component values.

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::diagnostic::{Diagnostic, DiagnosticCategory};

/// Minimum year representable by the canonical TypeDB date profile.
pub const MIN_CANONICAL_YEAR: i32 = -262_143;
/// Maximum year representable by the canonical TypeDB date profile.
pub const MAX_CANONICAL_YEAR: i32 = 262_142;

fn invalid_temporal(kind: &'static str) -> Diagnostic {
    Diagnostic::stable(
        DiagnosticCategory::InvalidContract,
        "invalid_canonical_scalar",
        "temporal value is outside its canonical grammar",
    )
    .with_detail("value_type", kind)
}

fn parse_digits(value: &str) -> Option<u32> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

/// A Gregorian date in the binding-neutral year range 1 through 9999.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalDate {
    year: i32,
    month: u8,
    day: u8,
}

impl CanonicalDate {
    /// Validate calendar components.
    pub fn new(year: i32, month: u8, day: u8) -> Result<Self, Diagnostic> {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let max_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => 0,
        };
        if !(MIN_CANONICAL_YEAR..=MAX_CANONICAL_YEAR).contains(&year) || day == 0 || day > max_day {
            Err(invalid_temporal("date"))
        } else {
            Ok(Self { year, month, day })
        }
    }
    /// Return `(year, month, day)`.
    pub const fn components(self) -> (i32, u8, u8) {
        (self.year, self.month, self.day)
    }
}
impl fmt::Display for CanonicalDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.year {
            0..=9999 => write!(f, "{:04}", self.year)?,
            10_000.. => write!(f, "+{}", self.year)?,
            -9999..=-1 => write!(f, "-{:04}", -self.year)?,
            _ => write!(f, "{}", self.year)?,
        }
        write!(f, "-{:02}-{:02}", self.month, self.day)
    }
}
impl FromStr for CanonicalDate {
    type Err = Diagnostic;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (year_month, day) = value
            .rsplit_once('-')
            .ok_or_else(|| invalid_temporal("date"))?;
        let (year, month) = year_month
            .rsplit_once('-')
            .ok_or_else(|| invalid_temporal("date"))?;
        let parsed = Self::new(
            year.parse::<i32>().map_err(|_| invalid_temporal("date"))?,
            parse_digits(month).ok_or_else(|| invalid_temporal("date"))? as u8,
            parse_digits(day).ok_or_else(|| invalid_temporal("date"))? as u8,
        )?;
        if parsed.to_string() != value {
            Err(invalid_temporal("date"))
        } else {
            Ok(parsed)
        }
    }
}

/// A time-of-day without timezone or leap seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalTime {
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
}

impl CanonicalTime {
    /// Validate time components.
    pub fn new(hour: u8, minute: u8, second: u8, nanosecond: u32) -> Result<Self, Diagnostic> {
        if hour > 23 || minute > 59 || second > 59 || nanosecond >= 1_000_000_000 {
            Err(invalid_temporal("datetime"))
        } else {
            Ok(Self {
                hour,
                minute,
                second,
                nanosecond,
            })
        }
    }
    /// Return `(hour, minute, second, nanosecond)`.
    pub const fn components(self) -> (u8, u8, u8, u32) {
        (self.hour, self.minute, self.second, self.nanosecond)
    }
}
impl fmt::Display for CanonicalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.nanosecond != 0 {
            let fraction = format!("{:09}", self.nanosecond);
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        Ok(())
    }
}
impl FromStr for CanonicalTime {
    type Err = Diagnostic;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !value.is_ascii() || value.len() < 8 || &value[2..3] != ":" || &value[5..6] != ":" {
            return Err(invalid_temporal("datetime"));
        }
        let hour = parse_digits(&value[..2]).ok_or_else(|| invalid_temporal("datetime"))? as u8;
        let minute = parse_digits(&value[3..5]).ok_or_else(|| invalid_temporal("datetime"))? as u8;
        let (seconds, nanos) = match value[6..].split_once('.') {
            Some((seconds, fraction))
                if !fraction.is_empty()
                    && fraction.len() <= 9
                    && fraction.bytes().all(|b| b.is_ascii_digit()) =>
            {
                let mut padded = fraction.to_owned();
                padded.extend(std::iter::repeat_n('0', 9 - fraction.len()));
                (
                    seconds,
                    padded
                        .parse::<u32>()
                        .map_err(|_| invalid_temporal("datetime"))?,
                )
            }
            Some(_) => return Err(invalid_temporal("datetime")),
            None => (&value[6..], 0),
        };
        Self::new(
            hour,
            minute,
            parse_digits(seconds).ok_or_else(|| invalid_temporal("datetime"))? as u8,
            nanos,
        )
    }
}

/// A timezone-free date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalDateTime {
    date: CanonicalDate,
    time: CanonicalTime,
}

impl CanonicalDateTime {
    /// Construct from validated components.
    pub const fn new(date: CanonicalDate, time: CanonicalTime) -> Self {
        Self { date, time }
    }
    /// Return the date component.
    pub const fn date(self) -> CanonicalDate {
        self.date
    }
    /// Return the time component.
    pub const fn time(self) -> CanonicalTime {
        self.time
    }
}
impl fmt::Display for CanonicalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}
impl FromStr for CanonicalDateTime {
    type Err = Diagnostic;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (date, time) = value
            .split_once('T')
            .ok_or_else(|| invalid_temporal("datetime"))?;
        if time.contains('T') {
            return Err(invalid_temporal("datetime"));
        }
        Ok(Self::new(date.parse()?, time.parse()?))
    }
}

/// A canonical timezone designator without timezone-database resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimeZoneDesignator {
    /// Coordinated Universal Time.
    Utc,
    /// A fixed signed offset from UTC in seconds.
    OffsetSeconds(i32),
    /// A validated named timezone identifier.
    Named(String),
}

/// A timezone-aware canonical date and time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalDateTimeTz {
    local: CanonicalDateTime,
    zone: TimeZoneDesignator,
    effective_offset_seconds: i32,
}

impl CanonicalDateTimeTz {
    /// Compatibility constructor for UTC and fixed-offset values.
    pub fn new(local: CanonicalDateTime, zone: TimeZoneDesignator) -> Result<Self, Diagnostic> {
        Self::new_fixed(local, zone)
    }

    /// Construct UTC or a fixed-offset value.
    pub fn new_fixed(
        local: CanonicalDateTime,
        zone: TimeZoneDesignator,
    ) -> Result<Self, Diagnostic> {
        match &zone {
            TimeZoneDesignator::Utc => Ok(Self {
                local,
                zone,
                effective_offset_seconds: 0,
            }),
            TimeZoneDesignator::OffsetSeconds(seconds)
                if seconds.unsigned_abs() <= 86_399 && *seconds != 0 =>
            {
                let effective_offset_seconds = *seconds;
                Ok(Self {
                    local,
                    zone,
                    effective_offset_seconds,
                })
            }
            TimeZoneDesignator::OffsetSeconds(_) | TimeZoneDesignator::Named(_) => {
                Err(invalid_temporal("datetime_tz"))
            }
        }
    }

    /// Construct a named-zone value with its provider-resolved effective offset.
    pub fn new_named_resolved(
        local: CanonicalDateTime,
        name: impl Into<String>,
        effective_offset_seconds: i32,
    ) -> Result<Self, Diagnostic> {
        let name = name.into();
        if name.is_empty()
            || name.len() > 255
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+')
            })
            || effective_offset_seconds.unsigned_abs() > 86_399
        {
            return Err(invalid_temporal("datetime_tz"));
        }
        Ok(Self {
            local,
            zone: TimeZoneDesignator::Named(name),
            effective_offset_seconds,
        })
    }
    /// Return the local date-time component.
    pub const fn local(&self) -> CanonicalDateTime {
        self.local
    }
    /// Return the written zone designator.
    pub fn zone(&self) -> &TimeZoneDesignator {
        &self.zone
    }
    /// Return the effective offset selected for this local value.
    pub const fn effective_offset_seconds(&self) -> i32 {
        self.effective_offset_seconds
    }

    /// Return the exact UTC instant as a timezone-independent nanosecond key.
    ///
    /// Provider-specific timezone resolution happens outside this low-dependency
    /// crate. Once resolved, this key is the semantic identity used by range
    /// comparison and schema fingerprints; the authored zone remains available
    /// through [`Self::zone`].
    pub fn semantic_utc_nanoseconds(&self) -> i128 {
        let (hour, minute, second, nanosecond) = self.local.time().components();
        let local_nanoseconds = i128::from(date_order_key(self.local.date())) * 86_400_000_000_000
            + i128::from(hour) * 3_600_000_000_000
            + i128::from(minute) * 60_000_000_000
            + i128::from(second) * 1_000_000_000
            + i128::from(nanosecond);
        local_nanoseconds - i128::from(self.effective_offset_seconds) * 1_000_000_000
    }
}

fn date_order_key(date: CanonicalDate) -> i64 {
    let (year, month, day) = date.components();
    let adjusted_year = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + i64::from(day) - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TimeZoneWire {
    Utc,
    OffsetSeconds { seconds: i32 },
    Named { name: String },
}

#[derive(Serialize, Deserialize)]
struct DateTimeTzWire {
    local: CanonicalDateTime,
    zone: TimeZoneWire,
    effective_offset_seconds: i32,
}

impl Serialize for CanonicalDateTimeTz {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let zone = match &self.zone {
            TimeZoneDesignator::Utc => TimeZoneWire::Utc,
            TimeZoneDesignator::OffsetSeconds(seconds) => {
                TimeZoneWire::OffsetSeconds { seconds: *seconds }
            }
            TimeZoneDesignator::Named(name) => TimeZoneWire::Named { name: name.clone() },
        };
        DateTimeTzWire {
            local: self.local,
            zone,
            effective_offset_seconds: self.effective_offset_seconds,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalDateTimeTz {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = DateTimeTzWire::deserialize(deserializer)?;
        match wire.zone {
            TimeZoneWire::Utc if wire.effective_offset_seconds == 0 => {
                Self::new_fixed(wire.local, TimeZoneDesignator::Utc).map_err(D::Error::custom)
            }
            TimeZoneWire::OffsetSeconds { seconds } if wire.effective_offset_seconds == seconds => {
                Self::new_fixed(wire.local, TimeZoneDesignator::OffsetSeconds(seconds))
                    .map_err(D::Error::custom)
            }
            TimeZoneWire::Named { name } => {
                Self::new_named_resolved(wire.local, name, wire.effective_offset_seconds)
                    .map_err(D::Error::custom)
            }
            TimeZoneWire::Utc | TimeZoneWire::OffsetSeconds { .. } => Err(D::Error::custom(
                "timezone offset does not match its authored fixed zone",
            )),
        }
    }
}
impl fmt::Display for CanonicalDateTimeTz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.local)?;
        match &self.zone {
            TimeZoneDesignator::Utc => f.write_str("Z"),
            TimeZoneDesignator::Named(name) => write!(f, "[{name}]"),
            TimeZoneDesignator::OffsetSeconds(seconds) => {
                let sign = if *seconds < 0 { '-' } else { '+' };
                let absolute = seconds.unsigned_abs();
                write!(
                    f,
                    "{sign}{:02}:{:02}",
                    absolute / 3600,
                    (absolute % 3600) / 60
                )
            }
        }
    }
}
impl FromStr for CanonicalDateTimeTz {
    type Err = Diagnostic;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !value.is_ascii() {
            return Err(invalid_temporal("datetime_tz"));
        }
        if let Some(local) = value.strip_suffix('Z') {
            return Self::new_fixed(local.parse()?, TimeZoneDesignator::Utc);
        }
        if value.ends_with(']') {
            return Err(invalid_temporal("datetime_tz"));
        }
        if value.len() < 6 {
            return Err(invalid_temporal("datetime_tz"));
        }
        let split = value.len() - 6;
        let offset = &value[split..];
        if !matches!(&offset[..1], "+" | "-") || &offset[3..4] != ":" {
            return Err(invalid_temporal("datetime_tz"));
        }
        let hours =
            parse_digits(&offset[1..3]).ok_or_else(|| invalid_temporal("datetime_tz"))? as i32;
        let minutes =
            parse_digits(&offset[4..]).ok_or_else(|| invalid_temporal("datetime_tz"))? as i32;
        if minutes > 59 {
            return Err(invalid_temporal("datetime_tz"));
        }
        let mut seconds = hours * 3600 + minutes * 60;
        if &offset[..1] == "-" {
            seconds = -seconds;
        }
        Self::new_fixed(
            value[..split].parse()?,
            TimeZoneDesignator::OffsetSeconds(seconds),
        )
    }
}

/// A normalized ISO-8601-subset duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalDuration {
    negative: bool,
    months: u64,
    days: u64,
    seconds: u64,
    nanosecond: u32,
}

impl CanonicalDuration {
    /// Construct a normalized duration from components.
    pub fn new(
        negative: bool,
        months: u64,
        days: u64,
        seconds: u64,
        nanosecond: u32,
    ) -> Result<Self, Diagnostic> {
        if nanosecond >= 1_000_000_000 {
            return Err(invalid_temporal("duration"));
        }
        let zero = months == 0 && days == 0 && seconds == 0 && nanosecond == 0;
        Ok(Self {
            negative: negative && !zero,
            months,
            days,
            seconds,
            nanosecond,
        })
    }
    /// Return `(negative, months, days, seconds, nanoseconds)`.
    pub const fn components(self) -> (bool, u64, u64, u64, u32) {
        (
            self.negative,
            self.months,
            self.days,
            self.seconds,
            self.nanosecond,
        )
    }
}
impl fmt::Display for CanonicalDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str("P")?;
        if self.months != 0 {
            write!(f, "{}M", self.months)?;
        }
        if self.days != 0 {
            write!(f, "{}D", self.days)?;
        }
        if self.seconds != 0 || self.nanosecond != 0 || (self.months == 0 && self.days == 0) {
            write!(f, "T{}", self.seconds)?;
            if self.nanosecond != 0 {
                let fraction = format!("{:09}", self.nanosecond);
                write!(f, ".{}", fraction.trim_end_matches('0'))?;
            }
            f.write_str("S")?;
        }
        Ok(())
    }
}

fn canonical_unsigned(value: &str) -> Option<u64> {
    let parsed = parse_digits(value)?.into();
    let parsed = value.parse::<u64>().ok().unwrap_or(parsed);
    (parsed.to_string() == value).then_some(parsed)
}

impl FromStr for CanonicalDuration {
    type Err = Diagnostic;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (negative, value) = value
            .strip_prefix('-')
            .map_or((false, value), |value| (true, value));
        let body = value
            .strip_prefix('P')
            .ok_or_else(|| invalid_temporal("duration"))?;
        if body == "T0S" {
            return Self::new(negative, 0, 0, 0, 0);
        }
        let (date, time) = body
            .split_once('T')
            .map_or((body, None), |(date, time)| (date, Some(time)));
        let mut months = 0;
        let mut days = 0;
        let mut rest = date;
        if let Some(index) = rest.find('M') {
            months =
                canonical_unsigned(&rest[..index]).ok_or_else(|| invalid_temporal("duration"))?;
            rest = &rest[index + 1..];
        }
        if let Some(index) = rest.find('D') {
            days =
                canonical_unsigned(&rest[..index]).ok_or_else(|| invalid_temporal("duration"))?;
            rest = &rest[index + 1..];
        }
        if !rest.is_empty() || months == 0 && date.contains('M') || days == 0 && date.contains('D')
        {
            return Err(invalid_temporal("duration"));
        }
        let (seconds, nanosecond) = if let Some(time) = time {
            let seconds = time
                .strip_suffix('S')
                .ok_or_else(|| invalid_temporal("duration"))?;
            match seconds.split_once('.') {
                Some((whole, fraction))
                    if !fraction.is_empty()
                        && fraction.len() <= 9
                        && !fraction.ends_with('0')
                        && fraction.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    let mut padded = fraction.to_owned();
                    padded.extend(std::iter::repeat_n('0', 9 - fraction.len()));
                    (
                        canonical_unsigned(whole).ok_or_else(|| invalid_temporal("duration"))?,
                        padded.parse().map_err(|_| invalid_temporal("duration"))?,
                    )
                }
                Some(_) => return Err(invalid_temporal("duration")),
                None => (
                    canonical_unsigned(seconds).ok_or_else(|| invalid_temporal("duration"))?,
                    0,
                ),
            }
        } else {
            (0, 0)
        };
        if months == 0 && days == 0 && seconds == 0 && nanosecond == 0 {
            return Err(invalid_temporal("duration"));
        }
        Self::new(negative, months, days, seconds, nanosecond)
    }
}

macro_rules! temporal_serde {
    ($type:ty) => {
        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(D::Error::custom)
            }
        }
    };
}
temporal_serde!(CanonicalDate);
temporal_serde!(CanonicalDateTime);
temporal_serde!(CanonicalDuration);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporal_values_parse_and_reemit_canonically() {
        assert_eq!(
            "2024-02-29".parse::<CanonicalDate>().unwrap().to_string(),
            "2024-02-29"
        );
        assert!("2023-02-29".parse::<CanonicalDate>().is_err());
        assert_eq!(
            "2024-01-02T03:04:05.1200"
                .parse::<CanonicalDateTime>()
                .unwrap()
                .to_string(),
            "2024-01-02T03:04:05.12"
        );
        assert_eq!(
            "2024-01-02T03:04:05Z"
                .parse::<CanonicalDateTimeTz>()
                .unwrap()
                .to_string(),
            "2024-01-02T03:04:05Z"
        );
        assert_eq!(
            "P2M3DT4.5S"
                .parse::<CanonicalDuration>()
                .unwrap()
                .to_string(),
            "P2M3DT4.5S"
        );
    }

    #[test]
    fn dates_cover_the_complete_provider_year_domain() {
        for value in [
            "-262143-01-01",
            "-0001-12-31",
            "0000-02-29",
            "+262142-12-31",
        ] {
            assert_eq!(value.parse::<CanonicalDate>().unwrap().to_string(), value);
        }
        for value in ["-262144-01-01", "+262143-01-01", "+0001-01-01", "1-01-01"] {
            assert!(
                value.parse::<CanonicalDate>().is_err(),
                "expected {value:?} to fail"
            );
        }
    }

    #[test]
    fn timezone_wire_retains_authored_zone_and_resolved_instant() {
        let local = "2024-10-27T01:30:00".parse::<CanonicalDateTime>().unwrap();
        let value = CanonicalDateTimeTz::new_named_resolved(local, "Europe/London", 3600).unwrap();
        let bytes = serde_json::to_string(&value).unwrap();
        assert_eq!(
            bytes,
            r#"{"local":"2024-10-27T01:30:00","zone":{"kind":"named","name":"Europe/London"},"effective_offset_seconds":3600}"#,
        );
        assert_eq!(
            serde_json::from_str::<CanonicalDateTimeTz>(&bytes).unwrap(),
            value
        );

        let utc = CanonicalDateTimeTz::new_fixed(
            "2024-01-01T12:00:00".parse().unwrap(),
            TimeZoneDesignator::Utc,
        )
        .unwrap();
        let offset = CanonicalDateTimeTz::new_fixed(
            "2024-01-01T13:00:00".parse().unwrap(),
            TimeZoneDesignator::OffsetSeconds(3600),
        )
        .unwrap();
        assert_eq!(
            utc.semantic_utc_nanoseconds(),
            offset.semantic_utc_nanoseconds()
        );

        let later_overlap =
            CanonicalDateTimeTz::new_named_resolved(local, "Europe/London", 0).unwrap();
        assert!(value.semantic_utc_nanoseconds() < later_overlap.semantic_utc_nanoseconds());
    }
}
