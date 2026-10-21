//! Private JNI boundary for generated Kotlin/JVM schema-package admission.

#![deny(missing_docs)]

mod error;
mod execution_diagnostic;
mod package;
mod scalar;

use jni::{
    Env, EnvUnowned, jni_sig, jni_str,
    objects::{JByteArray, JCharArray, JClass, JString},
};
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::limits::MAX_CANONICAL_BYTES;

use error::{JvmError, ThrowJvmError, allocation_error, diagnostic, invalid};

fn snapshot(env: &Env<'_>, input: &JByteArray<'_>, maximum: usize) -> Result<Vec<u8>, JvmError> {
    if input.is_null() {
        return Err(invalid(
            "jvm_null_input",
            "Native package inputs must not be null",
        ));
    }
    let length = input.len(env)?;
    if length > maximum {
        return Err(diagnostic(
            DiagnosticCategory::ResourceLimit,
            "jvm_input_limit",
            "Native package input exceeds its byte ceiling",
        )
        .into());
    }
    let mut bytes = Vec::<u8>::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| allocation_error())?;
    bytes.resize(length, 0);
    // SAFETY: u8 and i8 have identical layout and every bit pattern is valid.
    // The exclusive view ends before the owned UTF-8 bytes are inspected.
    let signed =
        unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr().cast::<i8>(), bytes.len()) };
    input.get_region(env, 0, signed)?;
    Ok(bytes)
}

fn snapshot_string(env: &mut Env<'_>, input: &JString<'_>) -> Result<String, JvmError> {
    use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
    if input.is_null() {
        return Err(invalid(
            "jvm_null_input",
            "Native string inputs must not be null",
        ));
    }
    // Bound before toCharArray allocates. A valid UTF-8 representation cannot
    // use fewer bytes than the number of UTF-16 code units in its JVM source.
    let length = env
        .call_method(input, jni_str!("length"), jni_sig!("()I"), &[])?
        .i()? as usize;
    if length > MAX_CANONICAL_STRING_BYTES {
        return Err(diagnostic(
            DiagnosticCategory::ResourceLimit,
            "jvm_input_limit",
            "Native string input exceeds its code-unit ceiling",
        )
        .into());
    }
    let array = env
        .call_method(input, jni_str!("toCharArray"), jni_sig!("()[C"), &[])?
        .l()?;
    let array = JCharArray::cast_local(env, array)?;
    let mut units = Vec::new();
    units
        .try_reserve_exact(length)
        .map_err(|_| allocation_error())?;
    units.resize(length, 0);
    array.get_region(env, 0, &mut units)?;
    let mut text = String::new();
    text.try_reserve_exact(length * 3)
        .map_err(|_| allocation_error())?;
    for character in char::decode_utf16(units) {
        text.push(character.map_err(|_| {
            invalid(
                "jvm_string_invalid_utf16",
                "Native string input contains an unpaired UTF-16 surrogate",
            )
        })?);
    }
    Ok(text)
}

/// Return the private JVM boundary version before any resource is admitted.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_abiVersion")]
pub extern "system" fn abi_version(mut env: EnvUnowned<'_>, _: JClass<'_>) -> i32 {
    env.with_env(|_| Ok::<_, JvmError>(3))
        .resolve::<ThrowJvmError>()
}

/// Return the engine-owned canonical resource byte ceiling.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_maximumEvidenceBytes")]
pub extern "system" fn maximum_evidence_bytes(mut env: EnvUnowned<'_>, _: JClass<'_>) -> i32 {
    env.with_env(|_| Ok::<_, JvmError>(MAX_CANONICAL_BYTES as i32))
        .resolve::<ThrowJvmError>()
}

/// Snapshot and admit one exact generated Kotlin/JVM package.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_openPackage")]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn open_package<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    namespace: JByteArray<'a>,
    authority: JByteArray<'a>,
    projection: JByteArray<'a>,
    semantic: JByteArray<'a>,
    binding: JByteArray<'a>,
    expected_authority: JByteArray<'a>,
    expected_binding: JByteArray<'a>,
) -> i64 {
    env.with_env(|env| -> Result<_, JvmError> {
        let namespace = snapshot(env, &namespace, 255)?;
        let authority = snapshot(env, &authority, MAX_CANONICAL_BYTES)?;
        let projection = snapshot(env, &projection, MAX_CANONICAL_BYTES)?;
        let semantic = snapshot(env, &semantic, MAX_CANONICAL_BYTES)?;
        let binding = snapshot(env, &binding, MAX_CANONICAL_BYTES)?;
        let expected_authority = snapshot(env, &expected_authority, MAX_CANONICAL_BYTES)?;
        let expected_binding = snapshot(env, &expected_binding, MAX_CANONICAL_BYTES)?;
        if binding != expected_binding {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "jvm_generated_identity_mismatch",
                "Package resources differ from the generated source identity",
            )
            .into());
        }
        let admitted = package::AdmittedPackage::open(
            &namespace,
            &authority,
            &projection,
            &semantic,
            &binding,
            &expected_authority,
        )?;
        package::insert(admitted)
    })
    .resolve::<ThrowJvmError>()
}

/// Return the verified package namespace while retaining an operation lease.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_namespace")]
pub extern "system" fn namespace<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
) -> JString<'a> {
    env.with_env(|env| -> Result<_, JvmError> {
        let package = package::lease(handle)?;
        Ok(JString::from_str(env, package.namespace())?)
    })
    .resolve::<ThrowJvmError>()
}

/// Return the number of canonical models in the admitted projection.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_modelCount")]
pub extern "system" fn model_count(mut env: EnvUnowned<'_>, _: JClass<'_>, handle: i64) -> i32 {
    env.with_env(|_| -> Result<_, JvmError> {
        i32::try_from(package::lease(handle)?.model_count()).map_err(|_| {
            invalid(
                "jvm_model_count_invalid",
                "Admitted model count exceeds the JVM range",
            )
        })
    })
    .resolve::<ThrowJvmError>()
}

/// Copy the exact canonical authority fingerprint into a caller-owned byte array.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_authorityFingerprint")]
pub extern "system" fn authority_fingerprint<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
) -> JByteArray<'a> {
    env.with_env(|env| -> Result<_, JvmError> {
        Ok(env.byte_array_from_slice(&package::lease(handle)?.authority_fingerprint()?)?)
    })
    .resolve::<ThrowJvmError>()
}

/// Copy the exact canonical semantic fingerprint into a caller-owned byte array.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_semanticFingerprint")]
pub extern "system" fn semantic_fingerprint<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
) -> JByteArray<'a> {
    env.with_env(|env| -> Result<_, JvmError> {
        Ok(env.byte_array_from_slice(&package::lease(handle)?.semantic_fingerprint()?)?)
    })
    .resolve::<ThrowJvmError>()
}

/// Copy the exact canonical binding fingerprint into a caller-owned byte array.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_bindingFingerprint")]
pub extern "system" fn binding_fingerprint<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
) -> JByteArray<'a> {
    env.with_env(|env| -> Result<_, JvmError> {
        Ok(env.byte_array_from_slice(&package::lease(handle)?.binding_fingerprint()?)?)
    })
    .resolve::<ThrowJvmError>()
}

/// Close admission to new operations; existing leases retain their owned state.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_closePackage")]
pub extern "system" fn close_package(mut env: EnvUnowned<'_>, _: JClass<'_>, handle: i64) {
    env.with_env(|_| package::close(handle))
        .resolve::<ThrowJvmError>()
}

/// Release retained immutable packages when the JVM unloads this native library.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnUnload(_: *mut jni::sys::JavaVM, _: *mut std::ffi::c_void) {
    if let Err(payload) = std::panic::catch_unwind(package::unload) {
        std::mem::forget(payload);
    }
}

#[cfg(test)]
#[path = "../../schema-codegen/tests/support/mod.rs"]
mod test_support;

/// Validate a generated integer attribute through the shared model authority.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_longAttribute")]
pub extern "system" fn long_attribute<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
    binding: JByteArray<'a>,
    label: JByteArray<'a>,
    value: i64,
) -> i64 {
    env.with_env(|env| -> Result<_, JvmError> {
        let binding = snapshot(env, &binding, MAX_CANONICAL_BYTES)?;
        let label = snapshot(env, &label, type_bridge_contract::id::MAX_LABEL_BYTES)?;
        scalar::validate(
            handle,
            &binding,
            &label,
            type_bridge_contract::value::CanonicalValue::Long(value),
        )?;
        Ok(value)
    })
    .resolve::<ThrowJvmError>()
}

/// Validate a generated Boolean attribute through the shared model authority.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_booleanAttribute")]
pub extern "system" fn boolean_attribute<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
    binding: JByteArray<'a>,
    label: JByteArray<'a>,
    value: bool,
) -> bool {
    env.with_env(|env| -> Result<_, JvmError> {
        let binding = snapshot(env, &binding, MAX_CANONICAL_BYTES)?;
        let label = snapshot(env, &label, type_bridge_contract::id::MAX_LABEL_BYTES)?;
        scalar::validate(
            handle,
            &binding,
            &label,
            type_bridge_contract::value::CanonicalValue::Boolean(value),
        )?;
        Ok(value)
    })
    .resolve::<ThrowJvmError>()
}

/// Validate a finite generated double while preserving its exact IEEE bits.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_doubleAttribute")]
pub extern "system" fn double_attribute<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
    binding: JByteArray<'a>,
    label: JByteArray<'a>,
    value: f64,
) -> f64 {
    env.with_env(|env| -> Result<_, JvmError> {
        let binding = snapshot(env, &binding, MAX_CANONICAL_BYTES)?;
        let label = snapshot(env, &label, type_bridge_contract::id::MAX_LABEL_BYTES)?;
        let canonical = type_bridge_contract::value::CanonicalDouble::new(value)?;
        scalar::validate(
            handle,
            &binding,
            &label,
            type_bridge_contract::value::CanonicalValue::Double(canonical),
        )?;
        Ok(value)
    })
    .resolve::<ThrowJvmError>()
}

/// Validate exact Unicode text through the shared canonical and model owners.
#[unsafe(export_name = "Java_io_github_ds1sqe_typebridge_NativeBridge_stringAttribute")]
pub extern "system" fn string_attribute<'a>(
    mut env: EnvUnowned<'a>,
    _: JClass<'a>,
    handle: i64,
    binding: JByteArray<'a>,
    label: JByteArray<'a>,
    value: JString<'a>,
) -> JString<'a> {
    env.with_env(|env| -> Result<_, JvmError> {
        let binding = snapshot(env, &binding, MAX_CANONICAL_BYTES)?;
        let label = snapshot(env, &label, type_bridge_contract::id::MAX_LABEL_BYTES)?;
        let text =
            type_bridge_contract::value::CanonicalString::new(snapshot_string(env, &value)?)?;
        scalar::validate(
            handle,
            &binding,
            &label,
            type_bridge_contract::value::CanonicalValue::String(text),
        )?;
        Ok(value)
    })
    .resolve::<ThrowJvmError>()
}
