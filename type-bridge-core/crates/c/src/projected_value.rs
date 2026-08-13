use std::fmt::{self, Write as _};
use std::mem::size_of;
use std::str::FromStr;
use std::sync::Arc;

use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, CanonicalTime,
    TimeZoneDesignator,
};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_schema::parse_provider_datetime_tz;

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus,
    borrowed_view, close_box, guarded, initialize_view,
};
use crate::allocation::{AllocationSite, allocation_exhausted, try_box};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_PROJECTED_TOKEN, GENERATED_INPUT_PROJECTED_VALUE,
    GENERATED_INPUT_SCHEMA_PACKAGE, direct_output_preflight,
};
use crate::projected_model::check_projected_attribute_value_ranges;
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};

/// Stable C kind for one canonical projected scalar value.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgeProjectedValueKind {
    /// UTF-8 string.
    String = 1,
    /// Signed 64-bit integer.
    Long = 2,
    /// Finite IEEE-754 binary64 bits.
    Double = 3,
    /// Boolean.
    Boolean = 4,
    /// Canonical date text.
    Date = 5,
    /// Canonical timezone-free date-time text.
    DateTime = 6,
    /// Canonical timezone-aware date-time text.
    DateTimeTz = 7,
    /// Canonical TypeDB decimal text.
    Decimal = 8,
    /// Canonical duration text.
    Duration = 9,
}

/// Opaque immutable package-branded canonical projected scalar.
pub struct TypeBridgeProjectedValue {
    pub(crate) package: Arc<SchemaPackageState>,
    pub(crate) value: Arc<type_bridge_orm::ProjectedAttributeValue>,
    canonical_text: Option<InlineCanonicalText>,
}

// The largest non-borrowable canonical spelling is a named-zone date-time:
// a signed six-digit year, full nanosecond time, brackets, and the contract's
// 255-byte IANA-name ceiling. Keeping it inline makes adopting an Arc-backed
// query scalar require only the already-reserved outer opaque handle.
const INLINE_CANONICAL_TEXT_CAPACITY: usize = 320;

struct InlineCanonicalText {
    bytes: [u8; INLINE_CANONICAL_TEXT_CAPACITY],
    len: usize,
}

impl InlineCanonicalText {
    fn new(value: &CanonicalValue) -> Option<Self> {
        let mut text = Self {
            bytes: [0; INLINE_CANONICAL_TEXT_CAPACITY],
            len: 0,
        };
        match value {
            CanonicalValue::Date(value) => write_date(&mut text, *value),
            CanonicalValue::DateTime(value) => write_datetime(&mut text, *value),
            CanonicalValue::DateTimeTz(value) => write_datetime_tz(&mut text, value),
            CanonicalValue::Duration(value) => write_duration(&mut text, *value),
            CanonicalValue::String(_)
            | CanonicalValue::Decimal(_)
            | CanonicalValue::Long(_)
            | CanonicalValue::Double(_)
            | CanonicalValue::Boolean(_) => return None,
        }
        .expect("canonical temporal spelling fits the fixed C handle buffer");
        Some(text)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn trim_ascii_zeroes(&mut self) {
        while self.len != 0 && self.bytes[self.len - 1] == b'0' {
            self.len -= 1;
        }
    }
}

impl fmt::Write for InlineCanonicalText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        let destination = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        destination.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn write_date(output: &mut InlineCanonicalText, value: CanonicalDate) -> fmt::Result {
    let (year, month, day) = value.components();
    match year {
        0..=9999 => write!(output, "{year:04}")?,
        10_000.. => write!(output, "+{year}")?,
        -9999..=-1 => write!(output, "-{magnitude:04}", magnitude = -year)?,
        _ => write!(output, "{year}")?,
    }
    write!(output, "-{month:02}-{day:02}")
}

fn write_time(output: &mut InlineCanonicalText, value: CanonicalTime) -> fmt::Result {
    let (hour, minute, second, nanosecond) = value.components();
    write!(output, "{hour:02}:{minute:02}:{second:02}")?;
    if nanosecond != 0 {
        write!(output, ".{nanosecond:09}")?;
        output.trim_ascii_zeroes();
    }
    Ok(())
}

fn write_datetime(output: &mut InlineCanonicalText, value: CanonicalDateTime) -> fmt::Result {
    write_date(output, value.date())?;
    output.write_char('T')?;
    write_time(output, value.time())
}

fn write_datetime_tz(output: &mut InlineCanonicalText, value: &CanonicalDateTimeTz) -> fmt::Result {
    write_datetime(output, value.local())?;
    match value.zone() {
        TimeZoneDesignator::Utc => output.write_char('Z'),
        TimeZoneDesignator::Named(name) => write!(output, "[{name}]"),
        TimeZoneDesignator::OffsetSeconds(seconds) => {
            let sign = if *seconds < 0 { '-' } else { '+' };
            let absolute = seconds.unsigned_abs();
            write!(
                output,
                "{sign}{hours:02}:{minutes:02}",
                hours = absolute / 3600,
                minutes = (absolute % 3600) / 60,
            )?;
            let seconds = absolute % 60;
            if seconds != 0 {
                write!(output, ":{seconds:02}")?;
            }
            Ok(())
        }
    }
}

fn write_duration(output: &mut InlineCanonicalText, value: CanonicalDuration) -> fmt::Result {
    let (negative, months, days, seconds, nanosecond) = value.components();
    if negative {
        output.write_char('-')?;
    }
    output.write_char('P')?;
    if months != 0 {
        write!(output, "{months}M")?;
    }
    if days != 0 {
        write!(output, "{days}D")?;
    }
    if seconds != 0 || nanosecond != 0 || (months == 0 && days == 0) {
        write!(output, "T{seconds}")?;
        if nanosecond != 0 {
            write!(output, ".{nanosecond:09}")?;
            output.trim_ascii_zeroes();
        }
        output.write_char('S')?;
    }
    Ok(())
}

impl TypeBridgeProjectedValue {
    pub(crate) fn from_owned(
        package: Arc<SchemaPackageState>,
        value: type_bridge_orm::ProjectedAttributeValue,
    ) -> Self {
        Self::from_arc(package, Arc::new(value))
    }

    pub(crate) fn from_arc(
        package: Arc<SchemaPackageState>,
        value: Arc<type_bridge_orm::ProjectedAttributeValue>,
    ) -> Self {
        let canonical_text = InlineCanonicalText::new(value.value());
        Self {
            package,
            value,
            canonical_text,
        }
    }

    pub(crate) fn package(&self) -> &Arc<SchemaPackageState> {
        &self.package
    }

    pub(crate) fn value(&self) -> &type_bridge_orm::ProjectedAttributeValue {
        &self.value
    }

    fn canonical_text(&self) -> Option<&[u8]> {
        match self.value.value() {
            CanonicalValue::String(value) => Some(value.as_str().as_bytes()),
            CanonicalValue::Decimal(value) => Some(value.as_str().as_bytes()),
            CanonicalValue::Date(_)
            | CanonicalValue::DateTime(_)
            | CanonicalValue::DateTimeTz(_)
            | CanonicalValue::Duration(_) => self
                .canonical_text
                .as_ref()
                .map(InlineCanonicalText::as_bytes),
            CanonicalValue::Long(_) | CanonicalValue::Double(_) | CanonicalValue::Boolean(_) => {
                None
            }
        }
    }

    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_projected_attribute_value_ranges(&self.value, preflight)
    }

    fn kind(&self) -> TypeBridgeProjectedValueKind {
        match self.value.value() {
            CanonicalValue::String(_) => TypeBridgeProjectedValueKind::String,
            CanonicalValue::Long(_) => TypeBridgeProjectedValueKind::Long,
            CanonicalValue::Double(_) => TypeBridgeProjectedValueKind::Double,
            CanonicalValue::Boolean(_) => TypeBridgeProjectedValueKind::Boolean,
            CanonicalValue::Date(_) => TypeBridgeProjectedValueKind::Date,
            CanonicalValue::DateTime(_) => TypeBridgeProjectedValueKind::DateTime,
            CanonicalValue::DateTimeTz(_) => TypeBridgeProjectedValueKind::DateTimeTz,
            CanonicalValue::Decimal(_) => TypeBridgeProjectedValueKind::Decimal,
            CanonicalValue::Duration(_) => TypeBridgeProjectedValueKind::Duration,
        }
    }
}

pub(crate) fn same_package_brand(
    left: &Arc<SchemaPackageState>,
    right: &Arc<SchemaPackageState>,
) -> bool {
    Arc::ptr_eq(left, right)
        || (left._projection.semantic_fingerprint() == right._projection.semantic_fingerprint()
            && left._projection.projection_fingerprint()
                == right._projection.projection_fingerprint())
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C projected-value code is valid")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C projected-value message is valid")
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn resource_limit() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        code("c_projected_value_text_limit_exceeded"),
        message("The projected scalar text exceeds its stable byte ceiling"),
    )
}

fn model_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_value_model_mismatch",
        "The projected scalar value does not match the generated attribute model token",
    )
}

unsafe fn preflight_value_output(
    value: *const TypeBridgeProjectedValue,
    output: *mut std::ffi::c_void,
    output_size: usize,
) -> Result<(), TypeBridgeStatus> {
    let preflight = direct_output_preflight(&[(output, output_size)])?;
    preflight.check_object_kind(GENERATED_INPUT_PROJECTED_VALUE, value.cast())?;
    if !value.is_null() {
        // SAFETY: the complete opaque value object is disjoint from the output.
        let value = unsafe { &*value };
        value.check_borrowed_ranges(&preflight)?;
    }
    Ok(())
}

unsafe fn copied_text(input: TypeBridgeByteView) -> Result<String, SdkExecutionDiagnostic> {
    if input.length > MAX_CANONICAL_STRING_BYTES {
        return Err(resource_limit());
    }
    if input.length != 0 && input.data.is_null() {
        return Err(invalid_input(
            "c_projected_value_text_view_invalid",
            "The projected scalar text view is invalid",
        ));
    }
    let bytes = if input.length == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `data..data+length` is readable for this
        // call. The stable ceiling bounds allocation and u8 has alignment one.
        unsafe { std::slice::from_raw_parts(input.data, input.length) }.to_vec()
    };
    String::from_utf8(bytes).map_err(|_| {
        invalid_input(
            "c_projected_value_text_utf8_invalid",
            "The projected scalar text is not valid UTF-8",
        )
    })
}

unsafe fn open_value(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    text_input: Option<TypeBridgeByteView>,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    build: impl FnOnce() -> Result<CanonicalValue, SdkExecutionDiagnostic>,
) -> TypeBridgeStatus {
    let outputs = [
        (out_value.cast(), size_of::<*mut TypeBridgeProjectedValue>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ];
    let preflight = match direct_output_preflight(&outputs) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for (kind, pointer) in [
        (GENERATED_INPUT_SCHEMA_PACKAGE, package.cast()),
        (GENERATED_INPUT_PROJECTED_TOKEN, attribute_token.cast()),
    ] {
        if let Err(status) = preflight.check_object_kind(kind, pointer) {
            return status;
        }
    }
    if !package.is_null() {
        // SAFETY: the complete package object was proven disjoint before dereference.
        let package = unsafe { &*package };
        if let Err(status) = preflight.check_package_borrowed_ranges(package.state()) {
            return status;
        }
    }
    if let Some(input) = text_input
        && let Err(status) = preflight.check_bytes(input.data.cast(), input.length)
    {
        return status;
    }
    // SAFETY: independent output initialization is the common constructor contract.
    if let Err(status) = unsafe { initialize_execution_outputs(out_value, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable package during this call.
        let package = unsafe { &*package };
        // SAFETY: generated token storage is readable for this call.
        let attribute_id = match unsafe { resolve_model_token(package.state(), attribute_token) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let canonical = match build() {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let value = match type_bridge_orm::ProjectedAttributeValue::try_new(
            &package.state().installed_projection,
            attribute_id,
            canonical,
        ) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let value = TypeBridgeProjectedValue::from_owned(Arc::clone(package.state()), value);
        let value = match try_box(AllocationSite::ProjectedValueHandle, value) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: output was initialized and remains caller-writable.
        unsafe { out_value.write_unaligned(Box::into_raw(value)) };
        TypeBridgeStatus::Ok
    })
}

macro_rules! lexical_constructor {
    ($function:ident, $type:ty, $variant:ident, $code:literal, $message:literal) => {
        #[doc = $message]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            package: *const TypeBridgeSchemaPackage,
            attribute_token: *const TypeBridgeProjectedTokenV1,
            input: TypeBridgeByteView,
            out_value: *mut *mut TypeBridgeProjectedValue,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: all caller pointers are validated and snapshotted by the shared constructor.
            unsafe {
                open_value(
                    package,
                    attribute_token,
                    Some(input),
                    out_value,
                    out_diagnostics,
                    || {
                        let text = copied_text(input)?;
                        <$type>::from_str(&text)
                            .map(CanonicalValue::$variant)
                            .map_err(|_| invalid_input($code, $message))
                    },
                )
            }
        }
    };
}

/// Construct a package-branded canonical UTF-8 string scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_string_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input: TypeBridgeByteView,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated and snapshotted by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            Some(input),
            out_value,
            out_diagnostics,
            || {
                let text = copied_text(input)?;
                CanonicalString::new(text)
                    .map(CanonicalValue::String)
                    .map_err(|_| resource_limit())
            },
        )
    }
}

/// Construct a package-branded canonical signed 64-bit scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_long_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input: i64,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            None,
            out_value,
            out_diagnostics,
            || Ok(CanonicalValue::Long(input)),
        )
    }
}

/// Construct a package-branded canonical finite binary64 scalar from exact bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_double_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input_bits: u64,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            None,
            out_value,
            out_diagnostics,
            || {
                CanonicalDouble::from_bits(input_bits)
                    .map(CanonicalValue::Double)
                    .map_err(|_| {
                        invalid_input(
                            "c_projected_value_double_non_finite",
                            "The projected double must contain finite IEEE binary64 bits",
                        )
                    })
            },
        )
    }
}

/// Construct a package-branded canonical Boolean scalar from exact zero or one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_boolean_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input: u8,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            None,
            out_value,
            out_diagnostics,
            || match input {
                0 => Ok(CanonicalValue::Boolean(false)),
                1 => Ok(CanonicalValue::Boolean(true)),
                _ => Err(invalid_input(
                    "c_projected_value_boolean_invalid",
                    "The projected Boolean must be exactly zero or one",
                )),
            },
        )
    }
}

lexical_constructor!(
    type_bridge_projected_value_date_open,
    CanonicalDate,
    Date,
    "c_projected_value_date_invalid",
    "The projected date must use canonical lexical form"
);
lexical_constructor!(
    type_bridge_projected_value_datetime_open,
    CanonicalDateTime,
    DateTime,
    "c_projected_value_datetime_invalid",
    "The projected date-time must use canonical lexical form"
);
/// Construct a package-branded canonical timezone-aware date-time scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_datetime_tz_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input: TypeBridgeByteView,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated and snapshotted by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            Some(input),
            out_value,
            out_diagnostics,
            || {
                let text = copied_text(input)?;
                parse_provider_datetime_tz(&text)
                    .map(CanonicalValue::DateTimeTz)
                    .map_err(|_| {
                        invalid_input(
                            "c_projected_value_datetime_tz_invalid",
                            "The projected timezone-aware date-time must use canonical lexical form",
                        )
                    })
            },
        )
    }
}

/// Construct a package-branded canonical decimal scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_decimal_open(
    package: *const TypeBridgeSchemaPackage,
    attribute_token: *const TypeBridgeProjectedTokenV1,
    input: TypeBridgeByteView,
    out_value: *mut *mut TypeBridgeProjectedValue,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller pointers are validated and snapshotted by the shared constructor.
    unsafe {
        open_value(
            package,
            attribute_token,
            Some(input),
            out_value,
            out_diagnostics,
            || {
                let text = copied_text(input)?;
                let decimal = DecimalValue::new(&text).map_err(|_| {
                    invalid_input(
                        "c_projected_value_decimal_invalid",
                        "The projected decimal must use canonical lexical form",
                    )
                })?;
                if decimal.as_str() != text {
                    return Err(invalid_input(
                        "c_projected_value_decimal_not_canonical",
                        "The projected decimal must already be normalized",
                    ));
                }
                Ok(CanonicalValue::Decimal(decimal))
            },
        )
    }
}

lexical_constructor!(
    type_bridge_projected_value_duration_open,
    CanonicalDuration,
    Duration,
    "c_projected_value_duration_invalid",
    "The projected duration must use canonical lexical form"
);

/// Validate a projected scalar against its generated exact attribute-model token.
///
/// This generated-only fence is read-only and performs no provider I/O.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_validate_model(
    value: *const TypeBridgeProjectedValue,
    model: *const TypeBridgeProjectedTokenV1,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    for (kind, pointer) in [
        (GENERATED_INPUT_PROJECTED_VALUE, value.cast()),
        (GENERATED_INPUT_PROJECTED_TOKEN, model.cast()),
    ] {
        if let Err(status) = preflight.check_object_kind(kind, pointer) {
            return status;
        }
    }
    if !value.is_null() {
        // SAFETY: the complete opaque value object is disjoint from the output.
        let value = unsafe { &*value };
        if let Err(status) = value.check_borrowed_ranges(&preflight) {
            return status;
        }
    }
    // SAFETY: preflight proved the diagnostics slot does not overlap a live input.
    unsafe { out_diagnostics.write_unaligned(std::ptr::null_mut()) };
    guarded(|| {
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains this immutable value for the call.
        let value = unsafe { &*value };
        // SAFETY: token storage remains readable and is resolved against the
        // exact immutable package retained by the scalar handle.
        let expected = match unsafe { resolve_model_token(&value.package, model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if &expected != value.value.attribute_type() {
            return return_execution_error(model_mismatch(), out_diagnostics);
        }
        TypeBridgeStatus::Ok
    })
}

/// Return the stable scalar kind from an immutable projected value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_kind(
    value: *const TypeBridgeProjectedValue,
    out_kind: *mut TypeBridgeProjectedValueKind,
) -> TypeBridgeStatus {
    // SAFETY: this is a read-only complete-range check before caller writes.
    if let Err(status) = unsafe {
        preflight_value_output(
            value,
            out_kind.cast(),
            size_of::<TypeBridgeProjectedValueKind>(),
        )
    } {
        return status;
    }
    guarded(|| {
        if out_kind.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied a writable output slot. String is a valid stable default.
        unsafe { out_kind.write_unaligned(TypeBridgeProjectedValueKind::String) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable value during this call.
        unsafe { out_kind.write_unaligned((&*value).kind()) };
        TypeBridgeStatus::Ok
    })
}

/// Borrow canonical text from a string, date, date-time, decimal, or duration value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_text(
    value: *const TypeBridgeProjectedValue,
    out_text: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    // SAFETY: this is a read-only complete-range check before caller writes.
    if let Err(status) =
        unsafe { preflight_value_output(value, out_text.cast(), size_of::<TypeBridgeByteView>()) }
    {
        return status;
    }
    guarded(|| {
        // SAFETY: initialize the output before inspecting the input handle.
        if let Err(status) = unsafe { initialize_view(out_text) } {
            return status;
        }
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable value during this call.
        let value = unsafe { &*value };
        let Some(text) = value.canonical_text() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match borrowed_view(text, out_text) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Return a canonical signed 64-bit scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_long(
    value: *const TypeBridgeProjectedValue,
    out_long: *mut i64,
) -> TypeBridgeStatus {
    // SAFETY: this is a read-only complete-range check before caller writes.
    if let Err(status) = unsafe { preflight_value_output(value, out_long.cast(), size_of::<i64>()) }
    {
        return status;
    }
    guarded(|| {
        if out_long.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied a writable output slot.
        unsafe { out_long.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable value during this call.
        match unsafe { (*value).value.value() } {
            CanonicalValue::Long(value) => {
                // SAFETY: output was validated above.
                unsafe { out_long.write_unaligned(*value) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Return exact finite IEEE binary64 bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_double_bits(
    value: *const TypeBridgeProjectedValue,
    out_bits: *mut u64,
) -> TypeBridgeStatus {
    // SAFETY: this is a read-only complete-range check before caller writes.
    if let Err(status) = unsafe { preflight_value_output(value, out_bits.cast(), size_of::<u64>()) }
    {
        return status;
    }
    guarded(|| {
        if out_bits.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied a writable output slot.
        unsafe { out_bits.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable value during this call.
        match unsafe { (*value).value.value() } {
            CanonicalValue::Double(value) => {
                // SAFETY: output was validated above.
                unsafe { out_bits.write_unaligned(value.bits()) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Return an exact zero-or-one Boolean scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_boolean(
    value: *const TypeBridgeProjectedValue,
    out_boolean: *mut u8,
) -> TypeBridgeStatus {
    // SAFETY: this is a read-only complete-range check before caller writes.
    if let Err(status) =
        unsafe { preflight_value_output(value, out_boolean.cast(), size_of::<u8>()) }
    {
        return status;
    }
    guarded(|| {
        if out_boolean.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller supplied a writable output slot.
        unsafe { out_boolean.write_unaligned(0) };
        if value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains a live immutable value during this call.
        match unsafe { (*value).value.value() } {
            CanonicalValue::Boolean(value) => {
                // SAFETY: output was validated above.
                unsafe { out_boolean.write_unaligned(u8::from(*value)) };
                TypeBridgeStatus::Ok
            }
            _ => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Close one Rust-owned projected scalar and clear its slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_projected_value_close(
    value: *mut *mut TypeBridgeProjectedValue,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented by the public header.
    unsafe { close_box(value) }
}

pub(crate) fn invalid_brand_diagnostic() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::generated_token_package_mismatch()
}

pub(crate) fn invalid_shape_diagnostic(
    code_value: &'static str,
    message_value: &'static str,
) -> SdkExecutionDiagnostic {
    invalid_input(code_value, message_value)
}

pub(crate) unsafe fn snapshot_handle_array<T>(
    values: *const *const T,
    length: usize,
) -> Result<Vec<*const T>, SdkExecutionDiagnostic> {
    if length > type_bridge_contract::limits::MAX_CANONICAL_COLLECTION_LEN {
        return Err(SdkExecutionDiagnostic::resource_limit(
            code("c_projected_handle_collection_limit_exceeded"),
            message("The projected handle collection exceeds its stable item ceiling"),
        ));
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    if values.is_null() {
        return Err(invalid_input(
            "c_projected_handle_array_invalid",
            "The projected handle array is invalid",
        ));
    }
    let mut output = Vec::with_capacity(length);
    for index in 0..length {
        // SAFETY: caller promises all `length` pointer elements are readable.
        // Unaligned reads admit hostile-but-readable pointer-array storage.
        output.push(unsafe { values.add(index).read_unaligned() });
    }
    Ok(output)
}
