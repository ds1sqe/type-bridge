use jni::{
    Env, JValue, jni_sig, jni_str,
    objects::{JObject, JString, JThrowable},
    strings::JNIStr,
};
use type_bridge_contract::fingerprint::{Fingerprint, FingerprintAlgorithm};
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticDetailValue, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};

use crate::error::{JvmError, invalid};

fn text<'a>(env: &mut Env<'a>, value: &str) -> Result<JObject<'a>, JvmError> {
    Ok(JString::from_str(env, value)?.into())
}
fn string_variant<'a>(
    env: &mut Env<'a>,
    class: &JNIStr,
    value: &str,
) -> Result<JObject<'a>, JvmError> {
    let value = text(env, value)?;
    Ok(env.new_object(
        class,
        jni_sig!("(Ljava/lang/String;)V"),
        &[JValue::Object(&value)],
    )?)
}
fn pair<'a>(
    env: &mut Env<'a>,
    class: &JNIStr,
    first: &str,
    second: &str,
) -> Result<JObject<'a>, JvmError> {
    let first = text(env, first)?;
    let second = text(env, second)?;
    Ok(env.new_object(
        class,
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
        &[JValue::Object(&first), JValue::Object(&second)],
    )?)
}
fn unsigned<'a>(env: &mut Env<'a>, class: &JNIStr, value: u64) -> Result<JObject<'a>, JvmError> {
    let value = string_variant(env, jni_str!("java/math/BigInteger"), &value.to_string())?;
    Ok(env.new_object(
        class,
        jni_sig!("(Ljava/math/BigInteger;)V"),
        &[JValue::Object(&value)],
    )?)
}
fn ordinal<'a>(env: &mut Env<'a>, class: &JNIStr, value: u16) -> Result<JObject<'a>, JvmError> {
    Ok(env.new_object(class, jni_sig!("(I)V"), &[JValue::Int(i32::from(value))])?)
}
fn type_identity<'a>(env: &mut Env<'a>, value: &TypeId) -> Result<JObject<'a>, JvmError> {
    let kind = match value.kind() {
        TypeKind::Attribute => "attribute",
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Struct => "struct",
    };
    pair(
        env,
        jni_str!("io/github/ds1sqe/typebridge/DiagnosticType"),
        kind,
        value.label().as_str(),
    )
}
fn field_identity<'a>(env: &mut Env<'a>, value: &OwnsFactId) -> Result<JObject<'a>, JvmError> {
    let owner = type_identity(env, value.owner())?;
    let attribute = text(env, value.attribute().label().as_str())?;
    Ok(env.new_object(
        jni_str!("io/github/ds1sqe/typebridge/DiagnosticField"),
        jni_sig!("(Lio/github/ds1sqe/typebridge/DiagnosticType;Ljava/lang/String;)V"),
        &[JValue::Object(&owner), JValue::Object(&attribute)],
    )?)
}
fn role_identity<'a>(env: &mut Env<'a>, value: &RoleId) -> Result<JObject<'a>, JvmError> {
    pair(
        env,
        jni_str!("io/github/ds1sqe/typebridge/DiagnosticRole"),
        value.declaring_relation().as_str(),
        value.label().as_str(),
    )
}
fn fingerprint<'a>(env: &mut Env<'a>, value: &Fingerprint) -> Result<JObject<'a>, JvmError> {
    let domain = text(env, value.domain().as_str())?;
    let algorithm = text(
        env,
        match value.algorithm() {
            FingerprintAlgorithm::Sha256 => "sha256",
        },
    )?;
    let canonicalization = text(env, value.canonicalization().as_str())?;
    let profile = match value.semantic_profile() {
        Some(value) => text(env, value.as_str())?,
        None => env.new_local_ref(JObject::null())?,
    };
    let digest = text(env, &value.digest().to_hex())?;
    Ok(env.new_object(jni_str!("io/github/ds1sqe/typebridge/DiagnosticFingerprint"), jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"), &[JValue::Object(&domain), JValue::Object(&algorithm), JValue::Object(&canonicalization), JValue::Object(&profile), JValue::Object(&digest)])?)
}

macro_rules! identity_variant {
    ($env:expr, $class:literal, $signature:literal, $identity:expr) => {{
        let identity = $identity?;
        Ok($env.new_object(
            jni_str!($class),
            jni_sig!($signature),
            &[JValue::Object(&identity)],
        )?)
    }};
}

fn path<'a>(env: &mut Env<'a>, value: &SdkDiagnosticPathSegment) -> Result<JObject<'a>, JvmError> {
    use SdkDiagnosticPathSegment as P;
    match value {
        P::Argument(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$Argument"),
            value.as_str(),
        ),
        P::Index(value) => unsigned(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$Index"),
            *value,
        ),
        P::Type(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticPath$Type",
            "(Lio/github/ds1sqe/typebridge/DiagnosticType;)V",
            type_identity(env, value)
        ),
        P::Field(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticPath$Field",
            "(Lio/github/ds1sqe/typebridge/DiagnosticField;)V",
            field_identity(env, value)
        ),
        P::Role(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticPath$Role",
            "(Lio/github/ds1sqe/typebridge/DiagnosticRole;)V",
            role_identity(env, value)
        ),
        P::Query(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$Query"),
            value.as_str(),
        ),
        P::QueryBinding(value) => ordinal(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$Binding"),
            *value,
        ),
        P::QueryField { owner, name } => pair(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$QueryField"),
            owner.as_str(),
            name.as_str(),
        ),
        P::QueryRole { owner, name } => pair(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$QueryRole"),
            owner.as_str(),
            name.as_str(),
        ),
        P::QueryRoleEdge(value) => ordinal(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$RoleEdge"),
            *value,
        ),
        P::QueryOutputSlot(value) => unsigned(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$OutputSlot"),
            *value,
        ),
        P::QueryOutputName(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$OutputName"),
            value.as_str(),
        ),
        P::ContractField(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$ContractField"),
            value.as_str(),
        ),
        P::ContractIdentity(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath$ContractIdentity"),
            value.as_str(),
        ),
        _ => Err(invalid(
            "jvm_diagnostic_variant_unsupported",
            "The JVM runtime does not recognize this diagnostic path variant",
        )),
    }
}

fn detail<'a>(
    env: &mut Env<'a>,
    value: &SdkDiagnosticDetailValue,
) -> Result<JObject<'a>, JvmError> {
    use SdkDiagnosticDetailValue as D;
    match value {
        D::Boolean(value) => Ok(env.new_object(
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$BooleanValue"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(*value)],
        )?),
        D::Count(value) => unsigned(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$Count"),
            *value,
        ),
        D::ByteCount(value) => unsigned(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$ByteCount"),
            *value,
        ),
        D::Signed(value) => Ok(env.new_object(
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$Signed"),
            jni_sig!("(J)V"),
            &[JValue::Long(*value)],
        )?),
        D::Capability(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$Capability"),
            value.as_str(),
        ),
        D::ValueType(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$ValueType"),
            value.as_str(),
        ),
        D::Type(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticDetail$Type",
            "(Lio/github/ds1sqe/typebridge/DiagnosticType;)V",
            type_identity(env, value)
        ),
        D::Field(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticDetail$Field",
            "(Lio/github/ds1sqe/typebridge/DiagnosticField;)V",
            field_identity(env, value)
        ),
        D::Role(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticDetail$Role",
            "(Lio/github/ds1sqe/typebridge/DiagnosticRole;)V",
            role_identity(env, value)
        ),
        D::Fingerprint(value) => identity_variant!(
            env,
            "io/github/ds1sqe/typebridge/DiagnosticDetail$Fingerprint",
            "(Lio/github/ds1sqe/typebridge/DiagnosticFingerprint;)V",
            fingerprint(env, value)
        ),
        D::ProviderOperation(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$ProviderOperation"),
            value.as_str(),
        ),
        D::CommitOutcome(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$CommitOutcome"),
            value.as_str(),
        ),
        D::QueryCategory(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$QueryCategory"),
            value.as_str(),
        ),
        D::QueryIdentity(value) => string_variant(
            env,
            jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$QueryIdentity"),
            value.as_str(),
        ),
        D::QueryIdentityList(values) => {
            let array = env.new_object_array(
                i32::try_from(values.len()).expect("SDK identity list is bounded"),
                jni_str!("java/lang/String"),
                JObject::null(),
            )?;
            for (index, value) in values.iter().enumerate() {
                let value = text(env, value.as_str())?;
                array.set_element(env, index, value)?;
            }
            Ok(env.new_object(
                jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetail$QueryIdentityList"),
                jni_sig!("([Ljava/lang/String;)V"),
                &[JValue::Object(array.as_ref())],
            )?)
        }
        _ => Err(invalid(
            "jvm_diagnostic_variant_unsupported",
            "The JVM runtime does not recognize this diagnostic detail variant",
        )),
    }
}

pub(crate) fn throw(env: &mut Env<'_>, value: &SdkExecutionDiagnostic) -> Result<(), JvmError> {
    env.with_local_frame(128, |env| -> Result<(), JvmError> {
        let paths = env.new_object_array(i32::try_from(value.path().len()).expect("SDK path is bounded"), jni_str!("io/github/ds1sqe/typebridge/DiagnosticPath"), JObject::null())?;
        for (index, value) in value.path().iter().enumerate() {
            let value = env.with_local_frame_returning_local::<_, JObject, JvmError>(64, |env| path(env, value))?;
            paths.set_element(env, index, value)?;
        }
        let details = env.new_object_array(i32::try_from(value.details().len()).expect("SDK details are bounded"), jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetailEntry"), JObject::null())?;
        for (index, (name, value)) in value.details().iter().enumerate() {
            let entry = env.with_local_frame_returning_local::<_, JObject, JvmError>(64, |env| -> Result<_, JvmError> {
                let name = text(env, name.as_str())?;
                let value = detail(env, value)?;
                Ok(env.new_object(jni_str!("io/github/ds1sqe/typebridge/DiagnosticDetailEntry"), jni_sig!("(Ljava/lang/String;Lio/github/ds1sqe/typebridge/DiagnosticDetail;)V"), &[JValue::Object(&name), JValue::Object(&value)])?)
            })?;
            details.set_element(env, index, entry)?;
        }
        let category = text(env, value.category().as_str())?;
        let code = text(env, value.code().as_str())?;
        let message = text(env, value.message().as_str())?;
        let diagnostic = env.new_object(jni_str!("io/github/ds1sqe/typebridge/ExecutionDiagnostic"), jni_sig!("(ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;[Lio/github/ds1sqe/typebridge/DiagnosticPath;[Lio/github/ds1sqe/typebridge/DiagnosticDetailEntry;)V"), &[JValue::Int(i32::from(value.version().get())), JValue::Object(&category), JValue::Object(&code), JValue::Object(&message), JValue::Object(paths.as_ref()), JValue::Object(details.as_ref())])?;
        let exception = env.new_object(jni_str!("io/github/ds1sqe/typebridge/SdkExecutionException"), jni_sig!("(Lio/github/ds1sqe/typebridge/ExecutionDiagnostic;)V"), &[JValue::Object(&diagnostic)])?;
        let exception = JThrowable::cast_local(env, exception)?;
        Ok(env.throw(exception)?)
    })
}

#[cfg(test)]
#[path = "execution_diagnostic_tests.rs"]
mod tests;
