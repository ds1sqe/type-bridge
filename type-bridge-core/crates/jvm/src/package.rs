use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::projection::{BindingTarget, KotlinJvmPackageName, RuntimeProjection};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_schema::{decode_schema_authority, schema_authority_capability_vocabulary};
use type_bridge_schema_codegen::verify_projection_evidence;

use crate::error::{JvmError, allocation_error, diagnostic, invalid};

pub(crate) struct AdmittedPackage {
    projection: Arc<RuntimeProjection>,
    installed: type_bridge_orm::InstalledRuntimeProjection,
    authority_fingerprint: type_bridge_contract::fingerprint::Fingerprint,
}
impl AdmittedPackage {
    pub(crate) fn open(
        namespace: &[u8],
        authority: &[u8],
        projection: &[u8],
        semantic: &[u8],
        binding: &[u8],
        expected_authority: &[u8],
    ) -> Result<Self, JvmError> {
        let namespace = std::str::from_utf8(namespace).map_err(|_| {
            invalid(
                "jvm_namespace_invalid",
                "Package namespace must be valid UTF-8",
            )
        })?;
        let namespace = KotlinJvmPackageName::new(namespace)?;
        let authority =
            decode_schema_authority(authority, &schema_authority_capability_vocabulary())?;
        if to_canonical_json(authority.authority_fingerprint())? != expected_authority {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "jvm_generated_identity_mismatch",
                "Package authority differs from the generated source identity",
            )
            .into());
        }
        let projection = decode_runtime_projection_verified(projection, semantic, binding)?;
        if projection.target() != BindingTarget::KotlinJvm
            || projection.config().kotlin_jvm_package_name() != Some(&namespace)
        {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "jvm_package_target_mismatch",
                "Package target or namespace differs from the generated entry point",
            )
            .into());
        }
        verify_projection_evidence(&authority, &projection)?;
        if projection.semantic_fingerprint() != authority.resolved_schema().semantic_fingerprint() {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "jvm_package_authority_mismatch",
                "Package projection and schema authority differ",
            )
            .into());
        }
        let installed = type_bridge_orm::InstalledRuntimeProjection::try_new(projection)
            .map_err(|_| {
                diagnostic(
                    DiagnosticCategory::Integrity,
                    "jvm_runtime_projection_invalid",
                    "Verified projection could not be installed in the model runtime",
                )
            })?
            .with_declared_schema_identity(
                authority
                    .resolved_schema()
                    .declared_identity_fingerprint()
                    .clone(),
            );
        Ok(Self {
            projection: installed.projection().clone(),
            installed,
            authority_fingerprint: authority.authority_fingerprint().clone(),
        })
    }

    pub(crate) fn installed(&self) -> &type_bridge_orm::InstalledRuntimeProjection {
        &self.installed
    }

    pub(crate) fn namespace(&self) -> &str {
        self.projection
            .config()
            .kotlin_jvm_package_name()
            .expect("admission validated the namespace")
            .as_str()
    }
    pub(crate) fn model_count(&self) -> usize {
        self.projection.models().len()
    }
    pub(crate) fn authority_fingerprint(&self) -> Result<Vec<u8>, JvmError> {
        Ok(to_canonical_json(&self.authority_fingerprint)?)
    }
    pub(crate) fn semantic_fingerprint(&self) -> Result<Vec<u8>, JvmError> {
        Ok(to_canonical_json(self.projection.semantic_fingerprint())?)
    }
    pub(crate) fn binding_fingerprint(&self) -> Result<Vec<u8>, JvmError> {
        Ok(to_canonical_json(self.projection.projection_fingerprint())?)
    }
}

struct PackageRegistry {
    next: i64,
    entries: HashMap<i64, Arc<AdmittedPackage>>,
}
static PACKAGES: OnceLock<Mutex<PackageRegistry>> = OnceLock::new();
fn registry() -> &'static Mutex<PackageRegistry> {
    PACKAGES.get_or_init(|| {
        Mutex::new(PackageRegistry {
            next: 1,
            entries: HashMap::new(),
        })
    })
}
fn poisoned() -> JvmError {
    invalid(
        "jvm_registry_unavailable",
        "Native package registry is unavailable",
    )
}

pub(crate) fn insert(package: AdmittedPackage) -> Result<i64, JvmError> {
    let mut guard = registry().lock().map_err(|_| poisoned())?;
    let id = guard.next;
    let next = id.checked_add(1).ok_or_else(allocation_error)?;
    guard
        .entries
        .try_reserve(1)
        .map_err(|_| allocation_error())?;
    guard.entries.insert(id, Arc::new(package));
    guard.next = next;
    Ok(id)
}
pub(crate) fn lease(id: i64) -> Result<Arc<AdmittedPackage>, JvmError> {
    registry()
        .lock()
        .map_err(|_| poisoned())?
        .entries
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            invalid(
                "jvm_package_closed",
                "Native package handle is invalid or closed",
            )
        })
}
pub(crate) fn close(id: i64) -> Result<(), JvmError> {
    let package = registry()
        .lock()
        .map_err(|_| poisoned())?
        .entries
        .remove(&id)
        .ok_or_else(|| {
            invalid(
                "jvm_package_closed",
                "Native package handle is invalid or closed",
            )
        })?;
    // Drop potentially large authority graphs outside the registry lock.
    drop(package);
    Ok(())
}
pub(crate) fn unload() {
    if let Some(registry) = PACKAGES.get() {
        let entries = match registry.lock() {
            Ok(mut guard) => std::mem::take(&mut guard.entries),
            Err(_) => return,
        };
        drop(entries);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support as support;

    use super::*;
    use type_bridge_contract::projection::ProjectionConfig;
    use type_bridge_schema::project;
    use type_bridge_schema_codegen::KotlinJvmEmitter;

    fn admitted() -> AdmittedPackage {
        let authority =
            support::authority("format: typebridge.schema/v2\nentities:\n  person: {}\n");
        let emitter = KotlinJvmEmitter::new();
        let projection = project(
            authority.resolved_schema(),
            BindingTarget::KotlinJvm,
            &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.people").unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let package = emitter.emit(&projection, &authority).unwrap();
        let stage = support::Stage::new();
        support::write_package(&package, stage.path());
        let root = stage
            .path()
            .join("src/main/resources/example/people/typebridge");
        let read = |name| std::fs::read(root.join(name)).unwrap();
        let expected_authority = to_canonical_json(authority.authority_fingerprint()).unwrap();
        let authority = read("schema-authority.json");
        let projection = read("runtime-projection.json");
        let semantic = read("semantic-fingerprint.json");
        let binding = read("binding-fingerprint.json");
        assert!(
            AdmittedPackage::open(
                b"example.other",
                &authority,
                &projection,
                &semantic,
                &binding,
                &expected_authority,
            )
            .is_err()
        );
        assert!(
            AdmittedPackage::open(
                b"example.people",
                b"{}",
                &projection,
                &semantic,
                &binding,
                &expected_authority
            )
            .is_err()
        );
        AdmittedPackage::open(
            b"example.people",
            &authority,
            &projection,
            &semantic,
            &binding,
            &expected_authority,
        )
        .unwrap()
    }

    #[test]
    fn leases_survive_close_but_handles_are_never_reused() {
        let id = insert(admitted()).unwrap();
        let retained = lease(id).unwrap();
        close(id).unwrap();
        assert!(lease(id).is_err());
        assert!(close(id).is_err());
        assert!(lease(-1).is_err());
        assert_eq!(retained.namespace(), "example.people");
        assert_eq!(retained.model_count(), 1);
        let next = insert(admitted()).unwrap();
        assert_ne!(id, next);
        close(next).unwrap();
    }
}
