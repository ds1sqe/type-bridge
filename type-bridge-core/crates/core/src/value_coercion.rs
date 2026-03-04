//! Value coercion and formatting for TypeDB value types.
//!
//! Converts JSON values to typed TypeDB values (string, long, double, boolean,
//! decimal, date, datetime, datetime-tz, duration) with validation and range
//! checking, and formats them as TypeQL literals.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A value that has been coerced to a specific TypeDB value type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoercedValue {
    /// The coerced JSON value.
    pub value: serde_json::Value,
    /// The TypeDB value type (e.g. `"string"`, `"long"`, `"datetime"`).
    pub value_type: String,
}

/// Error returned when value coercion fails.
#[derive(Debug, Clone)]
pub enum CoercionError {
    /// The value's type does not match the expected TypeDB type.
    TypeMismatch {
        /// String representation of the value.
        value: String,
        /// The expected TypeDB value type.
        expected: String,
        /// The actual type encountered.
        actual: String,
    },
    /// The value's format is invalid for the target type (e.g. bad date string).
    InvalidFormat {
        /// String representation of the value.
        value: String,
        /// The target TypeDB value type.
        expected_type: String,
        /// Human-readable description of the format error.
        message: String,
    },
    /// The value is outside the allowed range for the target type.
    OutOfRange {
        /// String representation of the value.
        value: String,
        /// The target TypeDB value type.
        expected_type: String,
        /// Human-readable description of the range violation.
        message: String,
    },
}

impl fmt::Display for CoercionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoercionError::TypeMismatch {
                value,
                expected,
                actual,
            } => write!(
                f,
                "Type mismatch: expected {}, got {} for value '{}'",
                expected, actual, value
            ),
            CoercionError::InvalidFormat {
                value,
                expected_type,
                message,
            } => write!(
                f,
                "Invalid {} format for '{}': {}",
                expected_type, value, message
            ),
            CoercionError::OutOfRange {
                value,
                expected_type,
                message,
            } => write!(
                f,
                "Value '{}' out of range for {}: {}",
                value, expected_type, message
            ),
        }
    }
}

impl std::error::Error for CoercionError {}

// ---------------------------------------------------------------------------
// String escaping (matches Python's format_value string handling exactly)
// ---------------------------------------------------------------------------

/// Format a string as a TypeQL quoted literal with JSON-style escaping.
/// Order matters: backslashes first, then other sequences.
pub fn format_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{}\"", escaped)
}

// ---------------------------------------------------------------------------
// Date/time/duration validators
// ---------------------------------------------------------------------------

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Parse exactly `n` digits from a string slice, returning the parsed number
/// and the remaining slice.
fn parse_digits(s: &str, n: usize) -> Option<(u32, &str)> {
    if s.len() < n {
        return None;
    }
    let (digits, rest) = s.split_at(n);
    digits.parse::<u32>().ok().map(|v| (v, rest))
}

/// Validate a date string in YYYY-MM-DD format.
/// Validate that a string is a well-formed ISO 8601 date (`YYYY-MM-DD`).
pub fn validate_date(s: &str) -> Result<(), String> {
    let (year, rest) = parse_digits(s, 4).ok_or("Expected 4-digit year")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after year".into());
    }
    let rest = &rest[1..];
    let (month, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit month")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after month".into());
    }
    let rest = &rest[1..];
    let (day, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit day")?;

    if !rest.is_empty() {
        return Err(format!("Unexpected trailing content: '{}'", rest));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("Month {} out of range 1-12", month));
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(format!(
            "Day {} out of range 1-{} for {}-{:02}",
            day, max_day, year, month
        ));
    }
    Ok(())
}

/// Parse time part: HH:MM:SS[.ffffff]. Returns remaining slice.
fn parse_time_part(s: &str) -> Result<&str, String> {
    let (hour, rest) = parse_digits(s, 2).ok_or("Expected 2-digit hour")?;
    if hour > 23 {
        return Err(format!("Hour {} out of range 0-23", hour));
    }
    if !rest.starts_with(':') {
        return Err("Expected ':' after hour".into());
    }
    let rest = &rest[1..];
    let (minute, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit minute")?;
    if minute > 59 {
        return Err(format!("Minute {} out of range 0-59", minute));
    }
    if !rest.starts_with(':') {
        return Err("Expected ':' after minute".into());
    }
    let rest = &rest[1..];
    let (second, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit second")?;
    if second > 59 {
        return Err(format!("Second {} out of range 0-59", second));
    }

    // Optional fractional seconds
    if let Some(rest) = rest.strip_prefix('.') {
        // Consume digits
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            return Err("Expected digits after '.'".into());
        }
        Ok(&rest[end..])
    } else {
        Ok(rest)
    }
}

/// Validate a datetime string: YYYY-MM-DDTHH:MM:SS[.ffffff]
/// Rejects if timezone offset is present.
/// Validate that a string is a well-formed ISO 8601 datetime (`YYYY-MM-DDThh:mm:ss[.fff]`).
pub fn validate_datetime(s: &str) -> Result<(), String> {
    // Parse date part
    let (year, rest) = parse_digits(s, 4).ok_or("Expected 4-digit year")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after year".into());
    }
    let rest = &rest[1..];
    let (month, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit month")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after month".into());
    }
    let rest = &rest[1..];
    let (day, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit day")?;

    if !(1..=12).contains(&month) {
        return Err(format!("Month {} out of range 1-12", month));
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(format!(
            "Day {} out of range 1-{} for {}-{:02}",
            day, max_day, year, month
        ));
    }

    if !rest.starts_with('T') {
        return Err("Expected 'T' between date and time".into());
    }
    let rest = &rest[1..];
    let rest = parse_time_part(rest)?;

    if rest.is_empty() {
        return Ok(());
    }
    // If there's timezone info, it's not a naive datetime
    if rest.starts_with('+') || rest.starts_with('-') || rest.starts_with('Z') {
        return Err("Naive datetime must not have timezone offset (use datetime-tz)".into());
    }
    Err(format!("Unexpected trailing content: '{}'", rest))
}

/// Validate a datetime-tz string: YYYY-MM-DDTHH:MM:SS[.ffffff](+HH:MM|-HH:MM|Z)
/// Requires timezone offset.
/// Validate that a string is a well-formed datetime with timezone offset
/// (`YYYY-MM-DDThh:mm:ss[.fff]+HH:MM` or `Z` suffix).
pub fn validate_datetime_tz(s: &str) -> Result<(), String> {
    // Parse date part
    let (year, rest) = parse_digits(s, 4).ok_or("Expected 4-digit year")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after year".into());
    }
    let rest = &rest[1..];
    let (month, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit month")?;
    if !rest.starts_with('-') {
        return Err("Expected '-' after month".into());
    }
    let rest = &rest[1..];
    let (day, rest) = parse_digits(rest, 2).ok_or("Expected 2-digit day")?;

    if !(1..=12).contains(&month) {
        return Err(format!("Month {} out of range 1-12", month));
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(format!(
            "Day {} out of range 1-{} for {}-{:02}",
            day, max_day, year, month
        ));
    }

    if !rest.starts_with('T') {
        return Err("Expected 'T' between date and time".into());
    }
    let rest = &rest[1..];
    let rest = parse_time_part(rest)?;

    // Must have timezone
    if rest.is_empty() {
        return Err("datetime-tz requires timezone offset".into());
    }
    if rest == "Z" || rest == "+00:00" || rest == "-00:00" {
        return Ok(());
    }
    if rest.starts_with('+') || rest.starts_with('-') {
        let tz_rest = &rest[1..];
        let (tz_hour, tz_rest) =
            parse_digits(tz_rest, 2).ok_or("Expected 2-digit timezone hour")?;
        if tz_hour > 23 {
            return Err(format!("Timezone hour {} out of range", tz_hour));
        }
        if !tz_rest.starts_with(':') {
            return Err("Expected ':' in timezone offset".into());
        }
        let tz_rest = &tz_rest[1..];
        let (tz_min, tz_rest) =
            parse_digits(tz_rest, 2).ok_or("Expected 2-digit timezone minute")?;
        if tz_min > 59 {
            return Err(format!("Timezone minute {} out of range", tz_min));
        }
        if !tz_rest.is_empty() {
            return Err(format!("Unexpected trailing content: '{}'", tz_rest));
        }
        return Ok(());
    }
    Err("datetime-tz requires timezone offset (+HH:MM, -HH:MM, or Z)".into())
}

/// Validate that a string is a well-formed ISO 8601 duration (`PnYnMnDTnHnMnS`).
pub fn validate_duration(s: &str) -> Result<(), String> {
    if !s.starts_with('P') {
        return Err("Duration must start with 'P'".into());
    }
    let rest = &s[1..];
    if rest.is_empty() {
        return Err("Duration must have at least one component".into());
    }

    let mut pos = rest;
    let mut has_component = false;
    let mut in_time = false;

    while !pos.is_empty() {
        if pos.starts_with('T') {
            if in_time {
                return Err("Duplicate 'T' in duration".into());
            }
            in_time = true;
            pos = &pos[1..];
            if pos.is_empty() {
                return Err("Expected time component after 'T'".into());
            }
            continue;
        }

        // Parse number (possibly fractional)
        let num_end = pos
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(pos.len());
        if num_end == 0 {
            return Err(format!("Unexpected character: '{}'", &pos[..1]));
        }
        let _num_str = &pos[..num_end];
        pos = &pos[num_end..];

        if pos.is_empty() {
            return Err("Expected unit designator (Y, M, D, H, S) after number".into());
        }
        let unit = pos.chars().next().unwrap();
        pos = &pos[1..];

        match unit {
            'Y' | 'D' if !in_time => has_component = true,
            'M' => has_component = true, // M is valid in both date and time
            'H' | 'S' if in_time => has_component = true,
            'Y' | 'D' if in_time => {
                return Err(format!("'{}' not allowed in time part of duration", unit));
            }
            'H' | 'S' if !in_time => {
                return Err(format!(
                    "'{}' only allowed in time part (after 'T') of duration",
                    unit
                ));
            }
            _ => return Err(format!("Unknown duration unit: '{}'", unit)),
        }
    }

    if !has_component {
        return Err("Duration must have at least one component".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ValueCoercer
// ---------------------------------------------------------------------------

/// Coerces JSON values to typed TypeDB values and formats them as TypeQL literals.
///
/// Supports optional per-attribute range constraints for numeric types.
pub struct ValueCoercer {
    range_constraints: HashMap<String, (Option<f64>, Option<f64>)>,
}

impl Default for ValueCoercer {
    fn default() -> Self {
        Self::new()
    }
}

impl ValueCoercer {
    /// Create a new coercer with no range constraints.
    pub fn new() -> Self {
        ValueCoercer {
            range_constraints: HashMap::new(),
        }
    }

    /// Add a range constraint for an attribute type.
    pub fn with_range(mut self, attr_type: &str, min: Option<f64>, max: Option<f64>) -> Self {
        self.range_constraints
            .insert(attr_type.to_string(), (min, max));
        self
    }

    /// Check range constraint for a numeric value.
    fn check_range(&self, value: f64, target_type: &str) -> Result<(), CoercionError> {
        if let Some((min, max)) = self.range_constraints.get(target_type) {
            if let Some(min_val) = min
                && value < *min_val
            {
                return Err(CoercionError::OutOfRange {
                    value: value.to_string(),
                    expected_type: target_type.to_string(),
                    message: format!("below minimum {}", min_val),
                });
            }
            if let Some(max_val) = max
                && value > *max_val
            {
                return Err(CoercionError::OutOfRange {
                    value: value.to_string(),
                    expected_type: target_type.to_string(),
                    message: format!("above maximum {}", max_val),
                });
            }
        }
        Ok(())
    }

    /// Coerce a JSON value to a target TypeDB value type.
    pub fn coerce(
        &self,
        value: &serde_json::Value,
        target_type: &str,
    ) -> Result<CoercedValue, CoercionError> {
        match target_type {
            "string" => self.coerce_to_string(value),
            "long" | "integer" => self.coerce_to_long(value, target_type),
            "double" => self.coerce_to_double(value),
            "boolean" => self.coerce_to_boolean(value),
            "decimal" => self.coerce_to_decimal(value),
            "date" => self.coerce_to_date(value),
            "datetime" => self.coerce_to_datetime(value),
            "datetime-tz" => self.coerce_to_datetime_tz(value),
            "duration" => self.coerce_to_duration(value),
            _ => Err(CoercionError::InvalidFormat {
                value: value.to_string(),
                expected_type: target_type.to_string(),
                message: format!("Unknown target type: {}", target_type),
            }),
        }
    }

    /// Batch coerce multiple (value, target_type) pairs.
    pub fn coerce_batch(
        &self,
        pairs: &[(serde_json::Value, String)],
    ) -> Vec<Result<CoercedValue, CoercionError>> {
        pairs
            .iter()
            .map(|(value, target_type)| self.coerce(value, target_type))
            .collect()
    }

    /// Format a CoercedValue as a TypeQL literal string.
    pub fn format_typeql(&self, coerced: &CoercedValue) -> String {
        match coerced.value_type.as_str() {
            "string" => format_string_literal(coerced.value.as_str().unwrap_or("")),
            "boolean" => {
                if coerced.value.as_bool().unwrap_or(false) {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            "long" | "integer" | "double" => {
                // For numbers stored as serde_json::Value
                if let Some(n) = coerced.value.as_i64() {
                    n.to_string()
                } else if let Some(n) = coerced.value.as_f64() {
                    // Ensure floats always have decimal point
                    let s = n.to_string();
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        s
                    } else {
                        format!("{}.0", s)
                    }
                } else {
                    coerced.value.to_string()
                }
            }
            "decimal" => {
                // Decimal: stored as string in JSON, formatted with 'dec' suffix
                if let Some(s) = coerced.value.as_str() {
                    format!("{}dec", s)
                } else {
                    format!("{}dec", coerced.value)
                }
            }
            "date" | "datetime" | "datetime-tz" | "duration" => {
                // These are stored as strings, output unquoted
                coerced.value.as_str().unwrap_or("").to_string()
            }
            _ => format_string_literal(&coerced.value.to_string()),
        }
    }

    /// Format a raw JSON value for TypeQL by inferring the type.
    /// This is the Rust equivalent of Python's format_value() for JSON values.
    pub fn format_value(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => format_string_literal(s),
            serde_json::Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Null => "\"None\"".to_string(),
            _ => format_string_literal(&value.to_string()),
        }
    }

    // -- Coercion implementations per type --

    fn coerce_to_string(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Null => "None".to_string(),
            _ => value.to_string(),
        };
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "string".to_string(),
        })
    }

    fn coerce_to_long(
        &self,
        value: &serde_json::Value,
        target_type: &str,
    ) -> Result<CoercedValue, CoercionError> {
        let n = match value {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i
                } else if n.as_f64().is_some() {
                    return Err(CoercionError::TypeMismatch {
                        value: value.to_string(),
                        expected: target_type.to_string(),
                        actual: "float".to_string(),
                    });
                } else {
                    return Err(CoercionError::InvalidFormat {
                        value: value.to_string(),
                        expected_type: target_type.to_string(),
                        message: "Cannot convert to integer".to_string(),
                    });
                }
            }
            serde_json::Value::String(s) => {
                s.parse::<i64>().map_err(|_| CoercionError::InvalidFormat {
                    value: s.clone(),
                    expected_type: target_type.to_string(),
                    message: "Cannot parse as integer".to_string(),
                })?
            }
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: target_type.to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        self.check_range(n as f64, target_type)?;
        Ok(CoercedValue {
            value: serde_json::json!(n),
            value_type: "long".to_string(),
        })
    }

    fn coerce_to_double(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let n = match value {
            serde_json::Value::Number(n) => {
                n.as_f64().ok_or_else(|| CoercionError::InvalidFormat {
                    value: value.to_string(),
                    expected_type: "double".to_string(),
                    message: "Cannot convert to float".to_string(),
                })?
            }
            serde_json::Value::String(s) => {
                s.parse::<f64>().map_err(|_| CoercionError::InvalidFormat {
                    value: s.clone(),
                    expected_type: "double".to_string(),
                    message: "Cannot parse as float".to_string(),
                })?
            }
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "double".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        self.check_range(n, "double")?;
        Ok(CoercedValue {
            value: serde_json::json!(n),
            value_type: "double".to_string(),
        })
    }

    fn coerce_to_boolean(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let b = match value {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(s) => match s.as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(CoercionError::InvalidFormat {
                        value: s.clone(),
                        expected_type: "boolean".to_string(),
                        message: "Expected 'true' or 'false'".to_string(),
                    });
                }
            },
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "boolean".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        Ok(CoercedValue {
            value: serde_json::json!(b),
            value_type: "boolean".to_string(),
        })
    }

    fn coerce_to_decimal(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => {
                // Strip optional 'dec' suffix
                let cleaned = s.strip_suffix("dec").unwrap_or(s);
                // Validate it looks like a decimal number
                if cleaned.is_empty() {
                    return Err(CoercionError::InvalidFormat {
                        value: s.clone(),
                        expected_type: "decimal".to_string(),
                        message: "Empty decimal value".to_string(),
                    });
                }
                let parse_part = if let Some(stripped) = cleaned.strip_prefix('-') {
                    stripped
                } else {
                    cleaned
                };
                if parse_part.is_empty()
                    || !parse_part.chars().all(|c| c.is_ascii_digit() || c == '.')
                {
                    return Err(CoercionError::InvalidFormat {
                        value: s.clone(),
                        expected_type: "decimal".to_string(),
                        message: "Invalid decimal format".to_string(),
                    });
                }
                cleaned.to_string()
            }
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "decimal".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "decimal".to_string(),
        })
    }

    fn coerce_to_date(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "date".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        validate_date(&s).map_err(|msg| CoercionError::InvalidFormat {
            value: s.clone(),
            expected_type: "date".to_string(),
            message: msg,
        })?;
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "date".to_string(),
        })
    }

    fn coerce_to_datetime(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "datetime".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        validate_datetime(&s).map_err(|msg| CoercionError::InvalidFormat {
            value: s.clone(),
            expected_type: "datetime".to_string(),
            message: msg,
        })?;
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "datetime".to_string(),
        })
    }

    fn coerce_to_datetime_tz(
        &self,
        value: &serde_json::Value,
    ) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "datetime-tz".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        validate_datetime_tz(&s).map_err(|msg| CoercionError::InvalidFormat {
            value: s.clone(),
            expected_type: "datetime-tz".to_string(),
            message: msg,
        })?;
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "datetime-tz".to_string(),
        })
    }

    fn coerce_to_duration(&self, value: &serde_json::Value) -> Result<CoercedValue, CoercionError> {
        let s = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => {
                return Err(CoercionError::TypeMismatch {
                    value: value.to_string(),
                    expected: "duration".to_string(),
                    actual: json_type_name(value).to_string(),
                });
            }
        };
        validate_duration(&s).map_err(|msg| CoercionError::InvalidFormat {
            value: s.clone(),
            expected_type: "duration".to_string(),
            message: msg,
        })?;
        Ok(CoercedValue {
            value: serde_json::Value::String(s),
            value_type: "duration".to_string(),
        })
    }
}

/// Helper: get a human-readable name for a JSON value type.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- String escaping tests --

    #[test]
    fn test_format_string_literal_simple() {
        assert_eq!(format_string_literal("hello"), "\"hello\"");
    }

    #[test]
    fn test_format_string_literal_quotes() {
        assert_eq!(
            format_string_literal("say \"hello\""),
            "\"say \\\"hello\\\"\""
        );
    }

    #[test]
    fn test_format_string_literal_backslashes() {
        assert_eq!(
            format_string_literal("path\\to\\file"),
            "\"path\\\\to\\\\file\""
        );
    }

    #[test]
    fn test_format_string_literal_newlines() {
        assert_eq!(format_string_literal("line1\nline2"), "\"line1\\nline2\"");
    }

    #[test]
    fn test_format_string_literal_tabs() {
        assert_eq!(format_string_literal("col1\tcol2"), "\"col1\\tcol2\"");
    }

    #[test]
    fn test_format_string_literal_carriage_return() {
        assert_eq!(format_string_literal("a\rb"), "\"a\\rb\"");
    }

    #[test]
    fn test_format_string_literal_empty() {
        assert_eq!(format_string_literal(""), "\"\"");
    }

    #[test]
    fn test_format_string_literal_unicode() {
        assert_eq!(format_string_literal("こんにちは"), "\"こんにちは\"");
    }

    #[test]
    fn test_format_string_literal_mixed_escapes() {
        // path\\to\\"file"
        assert_eq!(
            format_string_literal("path\\to\\\"file\""),
            "\"path\\\\to\\\\\\\"file\\\"\""
        );
    }

    // -- Date validation tests --

    #[test]
    fn test_validate_date_valid() {
        assert!(validate_date("2024-01-15").is_ok());
        assert!(validate_date("2024-12-31").is_ok());
        assert!(validate_date("2024-02-29").is_ok()); // Leap year
        assert!(validate_date("2000-02-29").is_ok()); // Century leap year
    }

    #[test]
    fn test_validate_date_invalid() {
        assert!(validate_date("2024-13-01").is_err()); // Month 13
        assert!(validate_date("2024-02-30").is_err()); // Feb 30
        assert!(validate_date("2023-02-29").is_err()); // Not leap year
        assert!(validate_date("2024-00-15").is_err()); // Month 0
        assert!(validate_date("2024-01-00").is_err()); // Day 0
        assert!(validate_date("2024-01-32").is_err()); // Day 32
        assert!(validate_date("1900-02-29").is_err()); // 1900 not leap year
    }

    // -- Datetime validation tests --

    #[test]
    fn test_validate_datetime_valid() {
        assert!(validate_datetime("2024-01-15T10:30:00").is_ok());
        assert!(validate_datetime("2024-01-15T10:30:00.123456").is_ok());
        assert!(validate_datetime("2024-01-15T00:00:00").is_ok());
        assert!(validate_datetime("2024-01-15T23:59:59").is_ok());
    }

    #[test]
    fn test_validate_datetime_rejects_timezone() {
        assert!(validate_datetime("2024-01-15T10:30:00+00:00").is_err());
        assert!(validate_datetime("2024-01-15T10:30:00Z").is_err());
        assert!(validate_datetime("2024-01-15T10:30:00-05:00").is_err());
    }

    // -- Datetime-tz validation tests --

    #[test]
    fn test_validate_datetime_tz_valid() {
        assert!(validate_datetime_tz("2024-01-15T10:30:00+00:00").is_ok());
        assert!(validate_datetime_tz("2024-01-15T10:30:00Z").is_ok());
        assert!(validate_datetime_tz("2024-01-15T10:30:00-05:00").is_ok());
        assert!(validate_datetime_tz("2024-01-15T10:30:00.123456+05:30").is_ok());
    }

    #[test]
    fn test_validate_datetime_tz_rejects_naive() {
        assert!(validate_datetime_tz("2024-01-15T10:30:00").is_err());
    }

    // -- Duration validation tests --

    #[test]
    fn test_validate_duration_valid() {
        assert!(validate_duration("P1D").is_ok());
        assert!(validate_duration("P1Y2M3D").is_ok());
        assert!(validate_duration("PT2H30M").is_ok());
        assert!(validate_duration("P1DT2H30M").is_ok());
        assert!(validate_duration("P1Y").is_ok());
        assert!(validate_duration("PT1S").is_ok());
        assert!(validate_duration("PT0.5S").is_ok());
        assert!(validate_duration("P1M").is_ok());
    }

    #[test]
    fn test_validate_duration_invalid() {
        assert!(validate_duration("P").is_err()); // No component
        assert!(validate_duration("1D").is_err()); // Missing P
        assert!(validate_duration("PT").is_err()); // T without component
        assert!(validate_duration("PH2").is_err()); // H without T
    }

    // -- Coercion tests --

    #[test]
    fn test_coerce_string() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!("hello"), "string").unwrap();
        assert_eq!(result.value, json!("hello"));
        assert_eq!(result.value_type, "string");

        // Number to string
        let result = c.coerce(&json!(42), "string").unwrap();
        assert_eq!(result.value, json!("42"));

        // Bool to string
        let result = c.coerce(&json!(true), "string").unwrap();
        assert_eq!(result.value, json!("true"));
    }

    #[test]
    fn test_coerce_long() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!(42), "long").unwrap();
        assert_eq!(result.value, json!(42));
        assert_eq!(result.value_type, "long");

        // String to long
        let result = c.coerce(&json!("123"), "long").unwrap();
        assert_eq!(result.value, json!(123));

        // Float to long should error
        assert!(c.coerce(&json!(3.15), "long").is_err());
    }

    #[test]
    fn test_coerce_double() {
        let c = ValueCoercer::new();
        // Float
        let result = c.coerce(&json!(3.15), "double").unwrap();
        assert_eq!(result.value_type, "double");

        // Int to double (valid coercion)
        let result = c.coerce(&json!(42), "double").unwrap();
        assert_eq!(result.value, json!(42.0));
        assert_eq!(result.value_type, "double");

        // String to double
        let result = c.coerce(&json!("3.15"), "double").unwrap();
        assert_eq!(result.value_type, "double");
    }

    #[test]
    fn test_coerce_boolean() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!(true), "boolean").unwrap();
        assert_eq!(result.value, json!(true));

        let result = c.coerce(&json!("false"), "boolean").unwrap();
        assert_eq!(result.value, json!(false));

        // Invalid string
        assert!(c.coerce(&json!("yes"), "boolean").is_err());
    }

    #[test]
    fn test_coerce_decimal() {
        let c = ValueCoercer::new();
        // Number to decimal
        let result = c.coerce(&json!(123.45), "decimal").unwrap();
        assert_eq!(result.value_type, "decimal");

        // String with dec suffix
        let result = c.coerce(&json!("123.45dec"), "decimal").unwrap();
        assert_eq!(result.value, json!("123.45"));

        // Plain string
        let result = c.coerce(&json!("100"), "decimal").unwrap();
        assert_eq!(result.value, json!("100"));
    }

    #[test]
    fn test_coerce_date() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!("2024-01-15"), "date").unwrap();
        assert_eq!(result.value, json!("2024-01-15"));
        assert_eq!(result.value_type, "date");

        // Invalid date
        assert!(c.coerce(&json!("2024-02-30"), "date").is_err());

        // Non-string
        assert!(c.coerce(&json!(42), "date").is_err());
    }

    #[test]
    fn test_coerce_datetime() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!("2024-01-15T10:30:00"), "datetime").unwrap();
        assert_eq!(result.value, json!("2024-01-15T10:30:00"));

        // With fractional seconds
        let result = c
            .coerce(&json!("2024-01-15T10:30:00.123456"), "datetime")
            .unwrap();
        assert_eq!(result.value, json!("2024-01-15T10:30:00.123456"));

        // Should reject timezone
        assert!(
            c.coerce(&json!("2024-01-15T10:30:00+00:00"), "datetime")
                .is_err()
        );
    }

    #[test]
    fn test_coerce_datetime_tz() {
        let c = ValueCoercer::new();
        let result = c
            .coerce(&json!("2024-01-15T10:30:00+00:00"), "datetime-tz")
            .unwrap();
        assert_eq!(result.value, json!("2024-01-15T10:30:00+00:00"));

        // Should reject naive
        assert!(
            c.coerce(&json!("2024-01-15T10:30:00"), "datetime-tz")
                .is_err()
        );
    }

    #[test]
    fn test_coerce_duration() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!("P1DT2H30M"), "duration").unwrap();
        assert_eq!(result.value, json!("P1DT2H30M"));

        // Invalid
        assert!(c.coerce(&json!("not-a-duration"), "duration").is_err());
    }

    #[test]
    fn test_coerce_with_range() {
        let c = ValueCoercer::new().with_range("long", Some(0.0), Some(100.0));
        assert!(c.coerce(&json!(50), "long").is_ok());
        assert!(c.coerce(&json!(101), "long").is_err());
        assert!(c.coerce(&json!(-1), "long").is_err());
    }

    #[test]
    fn test_coerce_batch() {
        let c = ValueCoercer::new();
        let pairs = vec![
            (json!("hello"), "string".to_string()),
            (json!(42), "long".to_string()),
            (json!("invalid"), "long".to_string()),
        ];
        let results = c.coerce_batch(&pairs);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());
    }

    // -- Format tests --

    #[test]
    fn test_format_typeql_string() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!("hello"),
            value_type: "string".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "\"hello\"");
    }

    #[test]
    fn test_format_typeql_boolean() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!(true),
            value_type: "boolean".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "true");
    }

    #[test]
    fn test_format_typeql_long() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!(42),
            value_type: "long".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "42");
    }

    #[test]
    fn test_format_typeql_decimal() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!("123.45"),
            value_type: "decimal".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "123.45dec");
    }

    #[test]
    fn test_format_typeql_date() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!("2024-01-15"),
            value_type: "date".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "2024-01-15");
    }

    #[test]
    fn test_format_typeql_datetime() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!("2024-01-15T10:30:00"),
            value_type: "datetime".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "2024-01-15T10:30:00");
    }

    #[test]
    fn test_format_typeql_duration() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!("P1DT2H30M"),
            value_type: "duration".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "P1DT2H30M");
    }

    #[test]
    fn test_format_value_infer() {
        let c = ValueCoercer::new();
        assert_eq!(c.format_value(&json!("hello")), "\"hello\"");
        assert_eq!(c.format_value(&json!(true)), "true");
        assert_eq!(c.format_value(&json!(false)), "false");
        assert_eq!(c.format_value(&json!(42)), "42");
        assert_eq!(c.format_value(&json!(3.15)), "3.15");
    }

    #[test]
    fn test_coerce_unknown_type() {
        let c = ValueCoercer::new();
        assert!(c.coerce(&json!("test"), "unknown_type").is_err());
    }

    #[test]
    fn test_coerce_integer_alias() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!(42), "integer").unwrap();
        assert_eq!(result.value, json!(42));
        assert_eq!(result.value_type, "long");
    }

    #[test]
    fn test_coerce_decimal_negative() {
        let c = ValueCoercer::new();
        let result = c.coerce(&json!("-50.25"), "decimal").unwrap();
        assert_eq!(result.value, json!("-50.25"));
    }

    #[test]
    fn test_format_typeql_double() {
        let c = ValueCoercer::new();
        let cv = CoercedValue {
            value: json!(3.15),
            value_type: "double".to_string(),
        };
        assert_eq!(c.format_typeql(&cv), "3.15");
    }
}
