//! Schema package markers and type-branded installation handshake.

use core::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::schema::encode_declared_schema;
use type_bridge_schema::{
    MAX_SCHEMA_AUTHORITY_BYTES, VerifiedSchemaAuthority, decode_schema_authority,
    schema_authority_capability_vocabulary,
};

use crate::error::{Error, Result};

#[doc(hidden)]
pub mod sealed {
    pub trait Sealed {}
}

/// A type-level marker representing a generated schema package.
pub trait Schema: sealed::Sealed + Send + Sync + 'static {}

/// Default marker representing an unbound database handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unbound;

impl sealed::Sealed for Unbound {}
impl Schema for Unbound {}

/// A generated schema package marker carrying fingerprint evidence branded by `S: Schema`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaPackage<S: Schema> {
    semantic_fingerprint_json: &'static str,
    projection_fingerprint_json: &'static str,
    runtime_projection_json: &'static str,
    declared_schema_json: Option<&'static str>,
    schema_authority_json: Option<&'static str>,
    managed_scope_id: Option<&'static str>,
    semantic_profile_id: Option<&'static str>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> SchemaPackage<S> {
    /// Construct a type-branded schema package marker from verified JSON evidence (generated-code SPI).
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        semantic_fingerprint_json: &'static str,
        projection_fingerprint_json: &'static str,
        runtime_projection_json: &'static str,
    ) -> Self {
        Self {
            semantic_fingerprint_json,
            projection_fingerprint_json,
            runtime_projection_json,
            declared_schema_json: None,
            schema_authority_json: None,
            managed_scope_id: None,
            semantic_profile_id: None,
            marker: PhantomData,
        }
    }

    /// Construct a generated package carrying canonical remote query
    /// authority in addition to verified runtime projection evidence.
    #[doc(hidden)]
    #[must_use]
    pub const fn new_with_declared(
        semantic_fingerprint_json: &'static str,
        projection_fingerprint_json: &'static str,
        runtime_projection_json: &'static str,
        declared_schema_json: &'static str,
    ) -> Self {
        Self {
            semantic_fingerprint_json,
            projection_fingerprint_json,
            runtime_projection_json,
            declared_schema_json: Some(declared_schema_json),
            schema_authority_json: None,
            managed_scope_id: None,
            semantic_profile_id: None,
            marker: PhantomData,
        }
    }

    /// Construct a generated package carrying one exact compiled schema
    /// authority and its constructor-extracted query inputs.
    #[doc(hidden)]
    #[must_use]
    pub const fn new_with_authority(
        semantic_fingerprint_json: &'static str,
        projection_fingerprint_json: &'static str,
        runtime_projection_json: &'static str,
        schema_authority_json: &'static str,
        declared_schema_json: &'static str,
        managed_scope_id: &'static str,
        semantic_profile_id: &'static str,
    ) -> Self {
        Self {
            semantic_fingerprint_json,
            projection_fingerprint_json,
            runtime_projection_json,
            declared_schema_json: Some(declared_schema_json),
            schema_authority_json: Some(schema_authority_json),
            managed_scope_id: Some(managed_scope_id),
            semantic_profile_id: Some(semantic_profile_id),
            marker: PhantomData,
        }
    }

    /// Perform offline fingerprint verification without connecting to a live server.
    pub fn verify(&self) -> Result<()> {
        let _ = self.verify_and_install_with_authority()?;
        Ok(())
    }

    /// Return the semantic schema fingerprint JSON string (generated-code SPI).
    #[doc(hidden)]
    #[must_use]
    pub const fn semantic_fingerprint_json(&self) -> &'static str {
        self.semantic_fingerprint_json
    }

    /// Return the binding target projection fingerprint JSON string (generated-code SPI).
    #[doc(hidden)]
    #[must_use]
    pub const fn projection_fingerprint_json(&self) -> &'static str {
        self.projection_fingerprint_json
    }

    /// Return the canonical runtime projection JSON string (generated-code SPI).
    #[doc(hidden)]
    #[must_use]
    pub const fn runtime_projection_json(&self) -> &'static str {
        self.runtime_projection_json
    }

    pub(crate) const fn declared_schema_json(&self) -> Option<&'static str> {
        self.declared_schema_json
    }

    /// Perform runtime fingerprint verification and derive provider descriptors (crate-internal).
    pub(crate) fn verify_and_install(
        &self,
    ) -> Result<Arc<type_bridge_orm::InstalledRuntimeProjection>> {
        self.verify_and_install_with_authority()
            .map(|(projection, _authority)| projection)
    }

    pub(crate) fn verify_and_install_with_authority(
        &self,
    ) -> Result<(
        Arc<type_bridge_orm::InstalledRuntimeProjection>,
        Option<VerifiedSchemaAuthority>,
    )> {
        let authority = self.verify_embedded_authority()?;
        let projection = type_bridge_orm::InstalledRuntimeProjection::from_verified_rust_json(
            self.runtime_projection_json.as_bytes(),
            self.semantic_fingerprint_json.as_bytes(),
            self.projection_fingerprint_json.as_bytes(),
        )
        .map_err(|err| Error::SchemaVerification {
            message: err.to_string(),
            source: Some(Box::new(err)),
        })?;
        if authority.as_ref().is_some_and(|authority| {
            authority.resolved_schema().semantic_fingerprint()
                != projection.projection().semantic_fingerprint()
        }) {
            return Err(authority_error(
                "generated schema authority does not match the installed runtime projection",
            ));
        }
        Ok((Arc::new(projection), authority))
    }

    fn verify_embedded_authority(&self) -> Result<Option<VerifiedSchemaAuthority>> {
        let parts = (
            self.schema_authority_json,
            self.declared_schema_json,
            self.managed_scope_id,
            self.semantic_profile_id,
        );
        let (Some(envelope), Some(declared), Some(scope), Some(profile)) = parts else {
            if parts.0.is_none() && parts.2.is_none() && parts.3.is_none() {
                return Ok(None);
            }
            return Err(authority_error(
                "generated schema package contains incomplete compiled authority evidence",
            ));
        };
        if envelope.len() > MAX_SCHEMA_AUTHORITY_BYTES {
            return Err(authority_error(
                "generated schema authority exceeds the canonical byte ceiling",
            ));
        }
        let authority = decode_schema_authority(
            envelope.as_bytes(),
            &schema_authority_capability_vocabulary(),
        )
        .map_err(|error| Error::SchemaVerification {
            message: format!(
                "generated schema package contains invalid compiled authority ({:?})",
                error.code()
            ),
            source: Some(Box::new(error)),
        })?;
        let reconstructed_declared =
            encode_declared_schema(authority.declared_schema()).map_err(|error| {
                Error::SchemaVerification {
                    message: "generated schema authority declaration cannot be reconstructed"
                        .into(),
                    source: Some(Box::new(error)),
                }
            })?;
        if reconstructed_declared != declared.as_bytes()
            || authority.managed_scope().id().as_str() != scope
            || authority.semantic_profile().id().as_str() != profile
        {
            return Err(authority_error(
                "generated schema authority disagrees with its extracted query evidence",
            ));
        }
        Ok(Some(authority))
    }
}

fn authority_error(message: &'static str) -> Error {
    Error::SchemaVerification {
        message: message.into(),
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::{
        CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
    };
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_schema::{
        ManagedDeltaContext, SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION,
        SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN, SchemaDocumentSet, build_schema_authority,
        encode_schema_authority, normalize_documents, project, resolve,
    };
    use type_bridge_schema_codegen::{PythonEmitter, RustEmitter};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestSchema;
    impl sealed::Sealed for TestSchema {}
    impl Schema for TestSchema {}

    fn leak(bytes: Vec<u8>) -> &'static str {
        Box::leak(String::from_utf8(bytes).unwrap().into_boxed_str())
    }

    fn generated_package(source: &str, scope: &str) -> SchemaPackage<TestSchema> {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("authority-test.yaml").unwrap(), source)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let authority = build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &ManagedDeltaContext::new(
                ManagedScopeId::new(scope).unwrap(),
                profile,
                schema_authority_capability_vocabulary(),
            ),
        )
        .unwrap();
        let emitter = RustEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        SchemaPackage::new_with_authority(
            leak(to_canonical_json(projection.semantic_fingerprint()).unwrap()),
            leak(to_canonical_json(projection.projection_fingerprint()).unwrap()),
            leak(to_canonical_json(&projection).unwrap()),
            leak(encode_schema_authority(&authority)),
            leak(encode_declared_schema(&declared).unwrap()),
            Box::leak(scope.to_owned().into_boxed_str()),
            "typedb-3.12.1/v1",
        )
    }

    fn with_envelope(
        package: SchemaPackage<TestSchema>,
        envelope: &'static str,
    ) -> SchemaPackage<TestSchema> {
        SchemaPackage::new_with_authority(
            package.semantic_fingerprint_json,
            package.projection_fingerprint_json,
            package.runtime_projection_json,
            envelope,
            package.declared_schema_json.unwrap(),
            package.managed_scope_id.unwrap(),
            package.semantic_profile_id.unwrap(),
        )
    }

    fn canonical(value: &Value) -> &'static str {
        leak(to_canonical_json(value).unwrap())
    }

    fn resign(value: &mut Value) {
        let content = to_canonical_json(&value["content"]).unwrap();
        let fingerprint = Fingerprint::compute(
            FingerprintDomain::new(SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN).unwrap(),
            CanonicalizationVersion::new(SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION).unwrap(),
            None,
            &content,
        );
        value["authority_fingerprint"] = serde_json::to_value(fingerprint).unwrap();
    }

    #[test]
    fn compiled_authority_is_fully_verified_and_bound_to_projection() {
        let package = generated_package(
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
            "rust-authority-test",
        );
        let (projection, authority) = package.verify_and_install_with_authority().unwrap();
        let authority = authority.expect("generated package has compiled authority");
        assert_eq!(
            projection.projection().semantic_fingerprint(),
            authority.resolved_schema().semantic_fingerprint(),
        );

        let foreign = generated_package(
            "format: typebridge.schema/v2\nentities:\n  organization: {}\n",
            "rust-authority-test",
        );
        let mismatched: SchemaPackage<TestSchema> = SchemaPackage::new_with_authority(
            package.semantic_fingerprint_json,
            package.projection_fingerprint_json,
            package.runtime_projection_json,
            foreign.schema_authority_json.unwrap(),
            foreign.declared_schema_json.unwrap(),
            foreign.managed_scope_id.unwrap(),
            foreign.semantic_profile_id.unwrap(),
        );
        let error = mismatched
            .verify()
            .expect_err("foreign semantic authority must not bind to the projection");
        assert!(error.to_string().contains("installed runtime projection"));
    }

    #[test]
    fn compiled_authority_rejects_missing_and_stale_outer_fingerprints() {
        let package = generated_package(
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
            "rust-authority-test",
        );
        let original: Value = serde_json::from_str(package.schema_authority_json.unwrap()).unwrap();

        let mut missing = original.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("authority_fingerprint");
        assert!(
            with_envelope(package, canonical(&missing))
                .verify()
                .is_err()
        );

        let mut stale = original;
        stale["authority_fingerprint"]["digest"] = "0".repeat(64).into();
        assert!(with_envelope(package, canonical(&stale)).verify().is_err());
    }

    #[test]
    fn compiled_authority_reconstructs_managed_state_and_rejects_unsupported_claims() {
        let package = generated_package(
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
            "rust-authority-test",
        );
        let original: Value = serde_json::from_str(package.schema_authority_json.unwrap()).unwrap();

        let mut managed = original.clone();
        managed["content"]["managed_state"]["declared_identity"]["digest"] = "0".repeat(64).into();
        resign(&mut managed);
        assert!(
            with_envelope(package, canonical(&managed))
                .verify()
                .is_err()
        );

        let mut capabilities = original.clone();
        capabilities["content"]["required_capabilities"] =
            Value::Array(vec![Value::String("unsupported.runtime".into())]);
        resign(&mut capabilities);
        let error = with_envelope(package, canonical(&capabilities))
            .verify()
            .expect_err("unsupported artifact capability must fail closed");
        assert!(error.to_string().contains("UnsupportedCapability"));

        let mut version = original;
        version["content"]["authority_version"] =
            Value::String("typebridge.schema-authority/v2".into());
        resign(&mut version);
        let error = with_envelope(package, canonical(&version))
            .verify()
            .expect_err("unsupported artifact version must fail closed");
        assert!(error.to_string().contains("UnsupportedVersion"));
    }

    #[test]
    fn compiled_authority_rejects_oversize_and_detached_evidence() {
        let package = generated_package(
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
            "rust-authority-test",
        );
        let oversized = Box::leak(" ".repeat(MAX_SCHEMA_AUTHORITY_BYTES + 1).into_boxed_str());
        let error = with_envelope(package, oversized)
            .verify()
            .expect_err("oversize authority must fail before parsing");
        assert!(error.to_string().contains("byte ceiling"));

        let detached: SchemaPackage<TestSchema> = SchemaPackage::new_with_authority(
            package.semantic_fingerprint_json,
            package.projection_fingerprint_json,
            package.runtime_projection_json,
            package.schema_authority_json.unwrap(),
            package.declared_schema_json.unwrap(),
            "other-scope",
            package.semantic_profile_id.unwrap(),
        );
        let error = detached
            .verify()
            .expect_err("detached scope must not override compiled authority");
        assert!(error.to_string().contains("extracted query evidence"));
    }

    #[test]
    fn schema_package_fingerprint_verification() {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("test.yaml").unwrap(),
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let emitter = RustEmitter::new();
        let resources = emitter.code_resources().unwrap();
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &emitter.generator_handlers(),
            &resources,
        )
        .unwrap();

        let semantic_json =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let projection_json =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();
        let runtime_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();

        let semantic_ref: &'static str = Box::leak(semantic_json.into_boxed_str());
        let projection_ref: &'static str = Box::leak(projection_json.into_boxed_str());
        let runtime_ref: &'static str = Box::leak(runtime_json.into_boxed_str());

        let valid_package: SchemaPackage<TestSchema> =
            SchemaPackage::new(semantic_ref, projection_ref, runtime_ref);
        assert!(valid_package.verify().is_ok());

        let tampered_package: SchemaPackage<TestSchema> =
            SchemaPackage::new(semantic_ref, r#""rust/v1-tampered""#, runtime_ref);
        match tampered_package.verify() {
            Err(err) => {
                use std::error::Error as _;
                assert!(err.source().is_some());
            }
            Ok(_) => panic!("tampered schema package must fail verification"),
        }
    }

    #[test]
    fn rejects_non_rust_target_projection() {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("test.yaml").unwrap(),
            "format: typebridge.schema/v2\nentities:\n  person: {}\n",
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let py_emitter = PythonEmitter::new();
        let py_resources = py_emitter.code_resources().unwrap();
        let py_projection = project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &py_emitter.generator_handlers(),
            &py_resources,
        )
        .unwrap();

        let semantic_json =
            String::from_utf8(to_canonical_json(py_projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let projection_json =
            String::from_utf8(to_canonical_json(py_projection.projection_fingerprint()).unwrap())
                .unwrap();
        let runtime_json = String::from_utf8(to_canonical_json(&py_projection).unwrap()).unwrap();

        let semantic_ref: &'static str = Box::leak(semantic_json.into_boxed_str());
        let projection_ref: &'static str = Box::leak(projection_json.into_boxed_str());
        let runtime_ref: &'static str = Box::leak(runtime_json.into_boxed_str());

        let py_package: SchemaPackage<TestSchema> =
            SchemaPackage::new(semantic_ref, projection_ref, runtime_ref);
        let err = py_package.verify().unwrap_err();
        assert!(err.to_string().contains("target mismatch") || err.to_string().contains("Rust"));
    }
}
