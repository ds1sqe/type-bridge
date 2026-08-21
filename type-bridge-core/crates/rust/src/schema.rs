//! Schema package markers and type-branded installation handshake.

use core::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::projection::{ProjectionConfig, RuntimeProjection};
use type_bridge_contract::schema::encode_declared_schema;
use type_bridge_contract::sdk_diagnostic::SdkProjectionEvidenceSlotPresence;
use type_bridge_schema::{
    MAX_SCHEMA_AUTHORITY_BYTES, VerifiedSchemaAuthority, decode_schema_authority,
    schema_authority_capability_vocabulary,
};
use type_bridge_schema_codegen::RustEmitter;

use crate::__codegen::{EncodedCreate, HydratedRow, ValidationError};
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

/// Opaque installed projection used by generated successor runtimes.
#[doc(hidden)]
pub struct GeneratedProjectionValidator {
    installed: Arc<type_bridge_orm::InstalledRuntimeProjection>,
}

impl GeneratedProjectionValidator {
    /// Validate one generated create encoding through the common projected model authority.
    pub fn validate_create(&self, encoded: &EncodedCreate) -> Result<(), ValidationError> {
        crate::projected_codec::validate_encoded_create(encoded, &self.installed)
            .map_err(generated_validation_error)
    }

    /// Validate one generated hydration row through the common projected model authority.
    pub fn validate_hydration(&self, row: &HydratedRow) -> Result<(), ValidationError> {
        crate::projected_codec::validate_hydrated_row(row, &self.installed)
            .map_err(generated_validation_error)
    }
}

fn generated_validation_error(error: Error) -> ValidationError {
    let code = error
        .code()
        .expect("generated projected validation always returns a classified model error")
        .to_owned();
    let path = error
        .path()
        .map(generated_validation_path)
        .unwrap_or_default();
    ValidationError::new(path, code)
}

fn generated_validation_path(segments: &[String]) -> String {
    let mut path = String::new();
    for segment in segments {
        if segment.starts_with('[') {
            path.push_str(segment);
        } else {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(segment);
        }
    }
    path
}

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

    /// Perform offline fingerprint, authority, and exact emitter-evidence verification
    /// without connecting to a live server.
    pub fn verify(&self) -> Result<()> {
        let _ = self.verify_and_install_with_authority()?;
        Ok(())
    }

    /// Verify this generated package and open an already schema-bound direct
    /// database through the canonical connection policy.
    ///
    /// Server compatibility is discovered authoritatively; callers cannot
    /// supply or override the server version used for admission.
    #[cfg(feature = "typedb")]
    pub async fn connect(
        self,
        policy: type_bridge_orm::DirectConnectionPolicy,
    ) -> Result<crate::session::Database<S>> {
        self.connect_with_cancellation(policy, type_bridge_orm::AnswerCancellation::default())
            .await
    }

    /// Verify this generated package and open an already schema-bound direct
    /// database with one wakeable connection-cancellation owner.
    #[cfg(feature = "typedb")]
    pub async fn connect_with_cancellation(
        self,
        policy: type_bridge_orm::DirectConnectionPolicy,
        cancellation: type_bridge_orm::AnswerCancellation,
    ) -> Result<crate::session::Database<S>> {
        let installed = self.verify_and_install()?;
        let match_registry = crate::session::build_match_registry(&installed)?;
        let inner =
            type_bridge_orm::Database::connect_direct(installed.as_ref(), &policy, cancellation)
                .await
                .map_err(Error::from_direct_connection)?;
        Ok(crate::session::Database::from_bound_parts(
            inner,
            installed,
            match_registry,
        ))
    }

    /// Install the package's exact projection for generated successor-runtime validation.
    /// Package verification remains projection-evidence admission; generated-token package
    /// fencing is provided by the nominal generated Rust types.
    #[doc(hidden)]
    pub fn generated_projection_validator(
        &self,
    ) -> std::result::Result<GeneratedProjectionValidator, ValidationError> {
        self.verify_and_install()
            .map(|installed| GeneratedProjectionValidator { installed })
            .map_err(|_| {
                ValidationError::new("projection_evidence", "projection_evidence_mismatch")
            })
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

    /// Verify all package evidence and derive provider descriptors without provider I/O
    /// (crate-internal).
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
        let authority_backed = self.schema_authority_json.is_some();
        // The detached semantic-fingerprint string is canonical evidence slot
        // zero. Empty bytes are the generated Rust install boundary's only
        // representable absence; every nonempty shape remains merely present
        // until the binding-neutral rejection classifier sees the failure.
        let semantic_fingerprint_presence = if self.semantic_fingerprint_json.is_empty() {
            SdkProjectionEvidenceSlotPresence::Absent
        } else {
            SdkProjectionEvidenceSlotPresence::Present
        };
        let authority = self.verify_embedded_authority().map_err(|error| {
            classify_admission_error(authority_backed, semantic_fingerprint_presence, error)
        })?;
        let projection = type_bridge_orm::InstalledRuntimeProjection::from_verified_rust_json(
            self.runtime_projection_json.as_bytes(),
            self.semantic_fingerprint_json.as_bytes(),
            self.projection_fingerprint_json.as_bytes(),
        )
        .map_err(|err| {
            classify_admission_error(
                authority_backed,
                semantic_fingerprint_presence,
                Error::SchemaVerification {
                    message: err.to_string(),
                    source: Some(Box::new(err)),
                },
            )
        })?;
        let successor =
            authority_backed || projection_uses_ordered_collections(projection.projection());
        if authority.as_ref().is_some_and(|authority| {
            authority.resolved_schema().semantic_fingerprint()
                != projection.projection().semantic_fingerprint()
        }) {
            return Err(classify_admission_error(
                successor,
                semantic_fingerprint_presence,
                authority_error(
                    "generated schema authority does not match the installed runtime projection",
                ),
            ));
        }
        verify_rust_projection_evidence(&projection, authority.as_ref()).map_err(|error| {
            classify_admission_error(successor, semantic_fingerprint_presence, error)
        })?;
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

fn verify_rust_projection_evidence(
    installed: &type_bridge_orm::InstalledRuntimeProjection,
    authority: Option<&VerifiedSchemaAuthority>,
) -> Result<()> {
    let projection = installed.projection();
    let emitter = RustEmitter::new();
    if projection.config() != &ProjectionConfig::rust() {
        return Err(authority_error(
            "generated Rust schema package does not match the exact shipped projection configuration",
        ));
    }
    if let Some(authority) = authority {
        return type_bridge_schema_codegen::verify_projection_evidence(authority, projection)
            .map_err(|error| Error::SchemaVerification {
                message: "generated Rust schema package does not match compiled schema authority and exact shipped emitter evidence"
                    .into(),
                source: Some(Box::new(error)),
            });
    }

    if projection_uses_ordered_collections(projection) {
        return Err(authority_error(
            "ordered Rust schema packages require compiled schema authority",
        ));
    }
    let resources = emitter
        .code_resources()
        .map_err(|error| Error::SchemaVerification {
            message: "legacy Rust schema package resource evidence cannot be reconstructed".into(),
            source: Some(Box::new(error)),
        })?;
    if projection.generator_handlers() != emitter.generator_handlers()
        || projection.code_resources() != resources
    {
        return Err(authority_error(
            "legacy Rust schema package does not match the exact shipped handler and resource evidence",
        ));
    }

    Ok(())
}

fn projection_uses_ordered_collections(projection: &RuntimeProjection) -> bool {
    projection.models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
}

fn authority_error(message: &'static str) -> Error {
    Error::SchemaVerification {
        message: message.into(),
        source: None,
    }
}

fn projection_evidence_error(presence: SdkProjectionEvidenceSlotPresence) -> Error {
    Error::projection_evidence_rejection(presence)
}

fn classify_admission_error(
    successor: bool,
    semantic_fingerprint_presence: SdkProjectionEvidenceSlotPresence,
    error: Error,
) -> Error {
    if successor {
        projection_evidence_error(semantic_fingerprint_presence)
    } else {
        error
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
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticPathSegment,
        SdkExecutionDiagnostic,
    };
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
        package_with_evidence(source, scope, None, None)
    }

    fn package_with_evidence(
        source: &str,
        scope: &str,
        handlers: Option<Vec<type_bridge_contract::projection::ProjectionHandler>>,
        resources: Option<Vec<type_bridge_contract::projection::CodeResourceDigest>>,
    ) -> SchemaPackage<TestSchema> {
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
        let handlers = handlers.unwrap_or_else(|| emitter.generator_handlers_for(&resolved));
        let resources = resources.unwrap_or_else(|| emitter.code_resources_for(&resolved).unwrap());
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &handlers,
            &resources,
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

    fn without_authority(package: SchemaPackage<TestSchema>) -> SchemaPackage<TestSchema> {
        SchemaPackage::new(
            package.semantic_fingerprint_json,
            package.projection_fingerprint_json,
            package.runtime_projection_json,
        )
    }

    fn with_runtime_projection(
        package: SchemaPackage<TestSchema>,
        runtime_projection_json: &'static str,
    ) -> SchemaPackage<TestSchema> {
        SchemaPackage::new_with_authority(
            package.semantic_fingerprint_json,
            package.projection_fingerprint_json,
            runtime_projection_json,
            package.schema_authority_json.unwrap(),
            package.declared_schema_json.unwrap(),
            package.managed_scope_id.unwrap(),
            package.semantic_profile_id.unwrap(),
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

    fn with_semantic_fingerprint(
        package: SchemaPackage<TestSchema>,
        semantic_fingerprint_json: &'static str,
    ) -> SchemaPackage<TestSchema> {
        SchemaPackage::new_with_authority(
            semantic_fingerprint_json,
            package.projection_fingerprint_json,
            package.runtime_projection_json,
            package.schema_authority_json.unwrap(),
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

    fn assert_projection_evidence_mismatch(error: &Error) {
        assert_eq!(error.category(), crate::ErrorCategory::Integrity);
        assert_eq!(error.model_validation_phase(), None);
        assert_eq!(error.code(), Some("projection_evidence_mismatch"));
        assert_eq!(
            error.path().expect("evidence mismatch has a stable path"),
            ["projection_evidence"],
        );
        assert_eq!(
            error.message(),
            "Generated projection evidence does not match the verified schema package",
        );
        assert!(matches!(
            error
                .diagnostic_path()
                .expect("evidence mismatch retains its typed path"),
            [crate::ErrorPathSegment::Argument(name)] if name == "projection_evidence"
        ));
        assert!(
            error
                .details()
                .expect("evidence mismatch retains typed details")
                .is_empty()
        );

        let diagnostic = std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<SdkExecutionDiagnostic>())
            .expect("the public Rust error retains the common SDK diagnostic");
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
        assert!(matches!(
            diagnostic.path(),
            [SdkDiagnosticPathSegment::Argument(name)]
                if name.as_str() == "projection_evidence"
        ));
        assert!(diagnostic.details().is_empty());
    }

    fn assert_missing_semantic_fingerprint(error: &Error) {
        assert_eq!(error.category(), crate::ErrorCategory::Integrity);
        assert_eq!(error.model_validation_phase(), None);
        assert_eq!(error.code(), Some("projection_evidence_mismatch"));
        assert_eq!(
            error.path().expect("missing evidence has a stable path"),
            ["projection_evidence", "[0]", "semantic_schema_fingerprint",],
        );
        assert!(matches!(
            error
                .diagnostic_path()
                .expect("missing evidence retains its typed path"),
            [
                crate::ErrorPathSegment::Argument(argument),
                crate::ErrorPathSegment::Index(0),
                crate::ErrorPathSegment::ContractIdentity(identity),
            ] if argument == "projection_evidence"
                && identity == "semantic_schema_fingerprint"
        ));
        assert_eq!(
            error
                .details()
                .expect("missing evidence retains typed details"),
            &std::collections::BTreeMap::from([
                (
                    "actual_occurrence_count".to_owned(),
                    crate::ErrorDetail::Long(0),
                ),
                (
                    "expected_occurrence_count".to_owned(),
                    crate::ErrorDetail::Long(1),
                ),
                (
                    "foreign_package".to_owned(),
                    crate::ErrorDetail::Boolean(false),
                ),
            ]),
        );

        let diagnostic = std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<SdkExecutionDiagnostic>())
            .expect("the Rust error retains the exact common SDK diagnostic");
        assert!(matches!(
            diagnostic.path(),
            [
                SdkDiagnosticPathSegment::Argument(argument),
                SdkDiagnosticPathSegment::Index(0),
                SdkDiagnosticPathSegment::ContractIdentity(identity),
            ] if argument.as_str() == "projection_evidence"
                && identity.as_str() == "semantic_schema_fingerprint"
        ));
        assert_eq!(
            diagnostic
                .details()
                .iter()
                .map(|(name, value)| (name.as_str(), value))
                .collect::<Vec<_>>(),
            vec![
                (
                    "actual_occurrence_count",
                    &SdkDiagnosticDetailValue::Count(0),
                ),
                (
                    "expected_occurrence_count",
                    &SdkDiagnosticDetailValue::Count(1),
                ),
                ("foreign_package", &SdkDiagnosticDetailValue::Boolean(false),),
            ],
        );
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
        assert_projection_evidence_mismatch(&error);
    }

    #[test]
    fn ordered_package_requires_exact_successor_handler_and_compiled_authority() {
        const ORDERED: &str = "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      tag:\n        card: 1\n        ordered: true\n        distinct: true\n";

        let valid = generated_package(ORDERED, "rust-ordered-evidence");
        let installed = valid.verify_and_install().unwrap();
        assert_eq!(
            installed.projection().generator_handlers(),
            [type_bridge_contract::projection::ProjectionHandler::rust_v2()],
        );
        let resource_ids = installed
            .projection()
            .code_resources()
            .iter()
            .map(|resource| resource.id().as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            resource_ids,
            [
                "typebridge.generator.rust.cargo-toml",
                "typebridge.generator.rust.runtime-source",
            ],
        );
        assert_ne!(
            installed.projection().code_resources(),
            RustEmitter::new().code_resources().unwrap(),
            "the ordered package must carry the successor runtime-resource digest",
        );

        let legacy = package_with_evidence(
            ORDERED,
            "rust-ordered-evidence",
            Some(vec![
                type_bridge_contract::projection::ProjectionHandler::rust_v1(),
            ]),
            Some(RustEmitter::new().code_resources().unwrap()),
        );
        let error = legacy
            .verify()
            .expect_err("an ordered package cannot claim the legacy Rust ledger");
        assert_projection_evidence_mismatch(&error);

        let authorityless = without_authority(valid);
        let error = authorityless
            .verify()
            .expect_err("ordered packages require reconstructable schema authority");
        assert_projection_evidence_mismatch(&error);

        let diagnostic = authorityless
            .generated_projection_validator()
            .err()
            .expect("ordered successor validation must reject the detached package");
        assert_eq!(diagnostic.code(), "projection_evidence_mismatch");
        assert_eq!(diagnostic.field(), "projection_evidence");
    }

    #[test]
    fn package_rejects_missing_forged_and_foreign_resource_evidence() {
        const UNORDERED: &str = "format: typebridge.schema/v2\nentities:\n  person: {}\n";
        const ORDERED: &str = "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      tag:\n        card: 1\n        ordered: true\n";

        let missing = package_with_evidence(
            ORDERED,
            "rust-resource-evidence",
            Some(vec![
                type_bridge_contract::projection::ProjectionHandler::rust_v2(),
            ]),
            Some(Vec::new()),
        );
        let error = missing
            .verify()
            .expect_err("ordered resource evidence is mandatory");
        assert_projection_evidence_mismatch(&error);

        let emitter = RustEmitter::new();
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("forged-resource.yaml").unwrap(), ORDERED)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let mut forged_resources = emitter.code_resources_for(&resolved).unwrap();
        let first_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] = type_bridge_contract::projection::CodeResourceDigest::from_bytes(
            first_id,
            b"forged Rust emitter resource",
        )
        .unwrap();
        let forged = package_with_evidence(
            ORDERED,
            "rust-resource-evidence",
            Some(emitter.generator_handlers_for(&resolved)),
            Some(forged_resources),
        );
        let error = forged
            .verify()
            .expect_err("self-consistent forged resource evidence must reject");
        assert_projection_evidence_mismatch(&error);

        let foreign_resources = type_bridge_schema_codegen::PythonEmitter::new()
            .code_resources_for(&resolved)
            .unwrap();
        let foreign = package_with_evidence(
            ORDERED,
            "rust-resource-evidence",
            Some(emitter.generator_handlers_for(&resolved)),
            Some(foreign_resources),
        );
        let error = foreign
            .verify()
            .expect_err("foreign binding resource evidence must reject");
        assert_projection_evidence_mismatch(&error);

        let valid_legacy = generated_package(UNORDERED, "rust-resource-evidence");
        let installed_legacy = valid_legacy.verify_and_install().unwrap();
        assert_eq!(
            installed_legacy.projection().generator_handlers(),
            [type_bridge_contract::projection::ProjectionHandler::rust_v1()],
        );
        assert_eq!(
            installed_legacy.projection().code_resources(),
            RustEmitter::new().code_resources().unwrap(),
        );
        assert!(without_authority(valid_legacy).verify().is_ok());
    }

    #[test]
    fn successor_admission_classifies_only_absent_detached_semantic_fingerprint() {
        let package = generated_package(
            "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      tag:\n        card: 1\n        ordered: true\n",
            "rust-missing-semantic-evidence",
        );
        let malformed = with_semantic_fingerprint(package, "{")
            .verify()
            .expect_err("nonempty malformed semantic evidence must reject");
        assert_projection_evidence_mismatch(&malformed);

        let error = with_semantic_fingerprint(package, "")
            .verify()
            .expect_err("an absent detached semantic fingerprint must reject");
        assert_missing_semantic_fingerprint(&error);
    }

    #[test]
    fn package_rejects_stale_and_reordered_evidence() {
        const UNORDERED: &str = "format: typebridge.schema/v2\nentities:\n  person: {}\n";
        const ORDERED: &str = "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      tag:\n        card: 1\n        ordered: true\n";

        let emitter = RustEmitter::new();
        let ordered_documents =
            SchemaDocumentSet::parse([(DocumentId::new("stale-resource.yaml").unwrap(), ORDERED)])
                .unwrap();
        let ordered_declared = normalize_documents(&ordered_documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let ordered_resolved = resolve(&ordered_declared, &profile).unwrap();
        let stale_successor = package_with_evidence(
            UNORDERED,
            "rust-stale-evidence",
            Some(emitter.generator_handlers_for(&ordered_resolved)),
            Some(emitter.code_resources_for(&ordered_resolved).unwrap()),
        );
        let error = stale_successor
            .verify()
            .expect_err("an unordered package cannot claim successor evidence");
        assert_projection_evidence_mismatch(&error);

        let ordered = generated_package(ORDERED, "rust-reordered-evidence");
        let mut runtime: Value = serde_json::from_str(ordered.runtime_projection_json).unwrap();
        runtime["code_resources"]
            .as_array_mut()
            .expect("runtime projection has a resource ledger")
            .swap(0, 1);
        let reordered = with_runtime_projection(ordered, canonical(&runtime));
        let error = reordered
            .verify()
            .expect_err("resource ledger wire order is canonical and cannot be changed");
        assert_projection_evidence_mismatch(&error);
    }

    #[test]
    fn authority_backed_admission_normalizes_malformed_extra_and_duplicate_evidence() {
        const ORDERED: &str = "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      tag:\n        card: 1\n        ordered: true\n";
        let package = generated_package(ORDERED, "rust-malformed-evidence");

        let malformed = with_envelope(package, "{")
            .verify()
            .expect_err("malformed authority evidence must reject");
        assert_projection_evidence_mismatch(&malformed);

        let runtime: Value = serde_json::from_str(package.runtime_projection_json).unwrap();
        let resource = runtime["code_resources"]
            .as_array()
            .and_then(|resources| resources.first())
            .cloned()
            .expect("successor projection carries resource evidence");

        let mut duplicate_runtime = runtime.clone();
        duplicate_runtime["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(resource.clone());
        let duplicate = with_runtime_projection(package, canonical(&duplicate_runtime))
            .verify()
            .expect_err("duplicate resource evidence must reject");
        assert_projection_evidence_mismatch(&duplicate);

        let mut extra_resource = resource;
        extra_resource["id"] = Value::String("typebridge.generator.rust.zzz-extra".into());
        let mut extra_runtime = runtime;
        extra_runtime["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(extra_resource);
        let extra = with_runtime_projection(package, canonical(&extra_runtime))
            .verify()
            .expect_err("extra resource evidence must reject");
        assert_projection_evidence_mismatch(&extra);
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
        let error = with_envelope(package, canonical(&missing))
            .verify()
            .expect_err("missing authority fingerprint must reject");
        assert_projection_evidence_mismatch(&error);

        let mut stale = original;
        stale["authority_fingerprint"]["digest"] = "0".repeat(64).into();
        let error = with_envelope(package, canonical(&stale))
            .verify()
            .expect_err("stale authority fingerprint must reject");
        assert_projection_evidence_mismatch(&error);
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
        let error = with_envelope(package, canonical(&managed))
            .verify()
            .expect_err("detached managed-state evidence must reject");
        assert_projection_evidence_mismatch(&error);

        let mut capabilities = original.clone();
        capabilities["content"]["required_capabilities"] =
            Value::Array(vec![Value::String("unsupported.runtime".into())]);
        resign(&mut capabilities);
        let error = with_envelope(package, canonical(&capabilities))
            .verify()
            .expect_err("unsupported artifact capability must fail closed");
        assert_projection_evidence_mismatch(&error);

        let mut version = original;
        version["content"]["authority_version"] =
            Value::String("typebridge.schema-authority/v2".into());
        resign(&mut version);
        let error = with_envelope(package, canonical(&version))
            .verify()
            .expect_err("unsupported artifact version must fail closed");
        assert_projection_evidence_mismatch(&error);
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
        assert_projection_evidence_mismatch(&error);

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
        assert_projection_evidence_mismatch(&error);
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
                assert_eq!(err.category(), crate::ErrorCategory::Schema);
                assert_eq!(err.code(), None);
                assert_eq!(err.path(), None);
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
        assert_eq!(err.category(), crate::ErrorCategory::Schema);
        assert_eq!(err.code(), None);
        assert_eq!(err.path(), None);
        assert!(err.to_string().contains("target mismatch") || err.to_string().contains("Rust"));
    }
}
