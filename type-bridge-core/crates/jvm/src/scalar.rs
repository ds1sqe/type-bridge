use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::ProjectedAttributeValue;

use crate::error::{JvmError, invalid};

pub(crate) fn validate(
    handle: i64,
    expected_binding: &[u8],
    label: &[u8],
    value: CanonicalValue,
) -> Result<ProjectedAttributeValue, JvmError> {
    let package = crate::package::lease(handle)?;
    if package.binding_fingerprint()? != expected_binding {
        return Err(SdkExecutionDiagnostic::generated_token_package_mismatch().into());
    }
    let label = std::str::from_utf8(label).map_err(|_| {
        invalid(
            "jvm_attribute_label_invalid",
            "Generated attribute label must be valid UTF-8",
        )
    })?;
    let id = TypeId::new(TypeKind::Attribute, label)?;
    Ok(ProjectedAttributeValue::try_new(
        package.installed(),
        id,
        value,
    )?)
}
