//! Schema package markers and type-branded installation handshake.

use core::marker::PhantomData;
use std::sync::Arc;

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
            marker: PhantomData,
        }
    }

    /// Perform offline fingerprint verification without connecting to a live server.
    pub fn verify(&self) -> Result<()> {
        let _ = type_bridge_orm::InstalledRuntimeProjection::from_verified_rust_json(
            self.runtime_projection_json.as_bytes(),
            self.semantic_fingerprint_json.as_bytes(),
            self.projection_fingerprint_json.as_bytes(),
        )
        .map_err(|err| Error::SchemaVerification {
            message: err.to_string(),
            source: Some(Box::new(err)),
        })?;
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
        let projection = type_bridge_orm::InstalledRuntimeProjection::from_verified_rust_json(
            self.runtime_projection_json.as_bytes(),
            self.semantic_fingerprint_json.as_bytes(),
            self.projection_fingerprint_json.as_bytes(),
        )
        .map_err(|err| Error::SchemaVerification {
            message: err.to_string(),
            source: Some(Box::new(err)),
        })?;
        Ok(Arc::new(projection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_codegen::{PythonEmitter, RustEmitter};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestSchema;
    impl sealed::Sealed for TestSchema {}
    impl Schema for TestSchema {}

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
