#![deny(missing_docs)]
//! Authenticated one-exchange remote execution for generated queries.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::{
    InstalledRuntimeProjection, RemoteModelQueryV2Error, ValidatedMatchRequest,
    ValidatedMatchResult, prepare_remote_model_query_v2,
};

use crate::Result;
use crate::error::Error;
use crate::query::QuerySession;
use crate::schema::{Schema, SchemaPackage, Unbound};

/// One caller-owned asynchronous transport for the authenticated V2 routes.
///
/// Implementations fetch the exact `/v2/capabilities` bytes once at connect
/// time and perform exactly one `/v2/query` exchange per terminal.
pub trait RemoteQueryTransport: Send + Sync + 'static {
    /// Fetch the executor's exact signed capability advertisement.
    fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>>;

    /// Exchange one exact canonical request for one exact signed reply.
    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>>;
}

/// Explicit immutable budgets for one remote generated-query terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteQueryLimits {
    limits: RemoteLimitsV2,
}

impl RemoteQueryLimits {
    /// Construct one explicit remote response and hydration budget.
    #[must_use]
    pub const fn new(
        max_items: u64,
        max_bytes: u64,
        max_collection_members: u64,
        max_graph_nodes: u64,
        max_attribute_values: u64,
        max_role_players: u64,
    ) -> Self {
        Self {
            limits: RemoteLimitsV2 {
                deadline_ms: None,
                max_bytes,
                max_items,
                max_collection_members,
                max_graph_nodes,
                max_attribute_values,
                max_role_players,
            },
        }
    }

    /// Attach an optional executor deadline in milliseconds.
    #[must_use]
    pub const fn deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.limits.deadline_ms = Some(deadline_ms);
        self
    }
}

/// Connection-time authority, transport, and limit configuration.
pub struct RemoteConnectionOptions {
    scope: String,
    semantic_profile: String,
    limits: RemoteQueryLimits,
    transport: Arc<dyn RemoteQueryTransport>,
    advertisement: Option<Vec<u8>>,
}

impl RemoteConnectionOptions {
    /// Construct remote options for one managed schema scope and semantic
    /// profile.
    #[must_use]
    pub fn new(
        scope: impl Into<String>,
        semantic_profile: impl Into<String>,
        limits: RemoteQueryLimits,
        transport: impl RemoteQueryTransport,
    ) -> Self {
        Self {
            scope: scope.into(),
            semantic_profile: semantic_profile.into(),
            limits,
            transport: Arc::new(transport),
            advertisement: None,
        }
    }
}

struct RemoteRuntime {
    advertisement: Vec<u8>,
    authority: Arc<QueryAuthority>,
    limits: RemoteLimitsV2,
    transport: Arc<dyn RemoteQueryTransport>,
}

/// A client-owned remote generated-query database branded by schema `S`.
pub struct RemoteDatabase<S: Schema = Unbound> {
    options: Option<RemoteConnectionOptions>,
    runtime: Option<Arc<RemoteRuntime>>,
    installed: Option<Arc<InstalledRuntimeProjection>>,
    registry: Option<Arc<DescriptorRegistry>>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> std::fmt::Debug for RemoteDatabase<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteDatabase")
            .field("schema_bound", &self.installed.is_some())
            .finish_non_exhaustive()
    }
}

impl RemoteDatabase<Unbound> {
    /// Fetch and validate one immutable executor advertisement.
    pub async fn connect(mut options: RemoteConnectionOptions) -> Result<Self> {
        let advertisement = options.transport.capabilities().await?;
        RemoteCapabilities::decode(&advertisement).map_err(remote_diagnostic)?;
        options.advertisement = Some(advertisement);
        Ok(Self {
            options: Some(options),
            runtime: None,
            installed: None,
            registry: None,
            marker: PhantomData,
        })
    }

    /// Verify and bind one generated schema package and its remote authority.
    pub fn with_schema<S: Schema>(mut self, schema: SchemaPackage<S>) -> Result<RemoteDatabase<S>> {
        let declared = schema
            .declared_schema_json()
            .ok_or_else(|| Error::SchemaVerification {
                message: "generated schema package omits remote declared-schema authority".into(),
                source: None,
            })?;
        let installed = schema.verify_and_install()?;
        let registry = Arc::new(installed.match_registry().map_err(Error::from_orm)?);
        let options = self.options.take().ok_or_else(|| Error::Other {
            message: "remote connection options are unavailable".into(),
            source: None,
        })?;
        let authority = QueryAuthority::from_declared_bytes(
            declared.as_bytes(),
            &options.scope,
            &options.semantic_profile,
        )
        .map_err(remote_diagnostic)?;
        if !authority.matches_semantic_fingerprint(installed.projection().semantic_fingerprint()) {
            return Err(Error::SchemaVerification {
                message: "remote declared-schema authority does not match the generated projection"
                    .into(),
                source: None,
            });
        }
        let runtime = Arc::new(RemoteRuntime {
            advertisement: options.advertisement.ok_or_else(|| Error::Other {
                message: "remote capability advertisement is unavailable".into(),
                source: None,
            })?,
            authority: Arc::new(authority),
            limits: options.limits.limits,
            transport: options.transport,
        });
        Ok(RemoteDatabase {
            options: None,
            runtime: Some(runtime),
            installed: Some(installed),
            registry: Some(registry),
            marker: PhantomData,
        })
    }
}

impl<S: Schema> RemoteDatabase<S> {
    /// Start one owner-branded query session over this remote executor.
    pub fn query(&self) -> Result<QuerySession<'_, S>> {
        let installed = self.installed.as_deref().ok_or_else(remote_not_bound)?;
        let registry = self.registry.as_ref().ok_or_else(remote_not_bound)?;
        Ok(QuerySession::remote(installed, Arc::clone(registry), self))
    }

    pub(crate) async fn execute_match(
        &self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult)> {
        let runtime = self.runtime.as_ref().ok_or_else(remote_not_bound)?;
        let pending = prepare_remote_model_query_v2(
            &runtime.authority,
            registry,
            validated,
            &runtime.advertisement,
            runtime.limits,
        )
        .map_err(remote_model_input_error)?;
        let request = pending.request_bytes().to_vec();
        let response = runtime.transport.exchange(&request).await?;
        let claimed = pending
            .claim_reply()
            .map_err(remote_model_hydration_error)?;
        if response.len() > claimed.response_snapshot_limit() {
            return Err(Error::classified(
                crate::ErrorCategory::ResourceLimit,
                None,
                "remote_response_limit",
                Vec::new(),
                "remote query reply exceeds the authenticated response ceiling",
                None,
            ));
        }
        let (request, result, _registry) = claimed
            .decode(&response)
            .map_err(remote_model_hydration_error)?;
        Ok((request, result))
    }
}

fn remote_not_bound() -> Error {
    Error::ModelValidation {
        phase: crate::ModelValidationPhase::Input,
        code: "schema_not_bound".into(),
        path: vec![],
        message: "remote database is not schema-bound".into(),
        source: None,
    }
}

fn remote_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    Error::from_remote_diagnostic(error)
}

fn remote_model_input_error(error: RemoteModelQueryV2Error) -> Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(error) => Error::from_remote_diagnostic(error),
        RemoteModelQueryV2Error::Match(error) => {
            Error::from_match(error, crate::ModelValidationPhase::Input)
        }
    }
}

fn remote_model_hydration_error(error: RemoteModelQueryV2Error) -> Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(error) => Error::from_remote_diagnostic(error),
        RemoteModelQueryV2Error::Match(error) => {
            Error::from_match(error, crate::ModelValidationPhase::Hydration)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPath, DiagnosticPathSegment,
    };
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::migration_assertion::BindingId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
    use type_bridge_contract::query_remote::RemoteExecutorBinding;
    use type_bridge_contract::query_remote_v2::{
        HydrationGraphV2, RemoteOutcomeV2, RemoteQueryFailureV2, RemoteQueryRequestV2,
        RemoteQueryResponseV2, RemoteResultKindV2, query_remote_v2_required_capabilities,
    };
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_orm::OrmError;
    use type_bridge_orm::match_request::SessionHandle;
    use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_codegen::RustEmitter;

    use super::*;
    use crate::__codegen::{
        self, CompleteModel, EncodedCreate, EntityModel, HydratedRow, HydrationCapability,
        IntoEncodedCreate, MaterializeModel, Model, ThingModel, ValidationError,
    };
    use crate::schema::sealed;

    struct TestSchema;
    impl sealed::Sealed for TestSchema {}
    impl Schema for TestSchema {}

    #[derive(Debug)]
    struct Person;
    impl sealed::Sealed for Person {}
    impl Model for Person {
        type Schema = TestSchema;
        const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"person"}"#;
    }
    impl ThingModel for Person {
        fn thing_kind() -> __codegen::ThingKind {
            __codegen::ThingKind::Entity
        }
    }
    impl EntityModel for Person {}
    impl CompleteModel for Person {
        type Create = PersonCreate;

        fn iid(&self) -> &str {
            unreachable!()
        }
    }
    impl MaterializeModel for Person {
        fn materialize(
            _: &HydratedRow,
            _: &HydrationCapability,
        ) -> std::result::Result<Self, ValidationError> {
            Ok(Self)
        }
    }

    #[derive(Clone)]
    struct PersonCreate;
    impl sealed::Sealed for PersonCreate {}
    impl IntoEncodedCreate for PersonCreate {
        fn into_encoded_create(self) -> std::result::Result<EncodedCreate, ValidationError> {
            Ok(EncodedCreate::new(Person::TYPE_ID_JSON, vec![], vec![]))
        }
    }

    #[test]
    fn local_and_remote_failures_preserve_classification_codes_and_paths() {
        let session = SessionHandle::new(Arc::new(DescriptorRegistry::new()));
        let match_error = match session.exact("missing") {
            Err(OrmError::Match(error)) => error,
            Err(other) => panic!("unexpected ORM error: {other:?}"),
            Ok(_) => panic!("missing descriptor unexpectedly resolved"),
        };
        let local = Error::from_orm(OrmError::Match(match_error.clone()));
        let remote = remote_model_input_error(RemoteModelQueryV2Error::Match(match_error));

        assert_eq!(local.category(), crate::ErrorCategory::QueryAuthoring);
        assert_eq!(remote.category(), local.category());
        assert_eq!(remote.code(), Some("unknown_descriptor"));
        assert_eq!(remote.code(), local.code());
        assert_eq!(remote.path(), local.path());
        assert_eq!(
            remote.model_validation_phase(),
            local.model_validation_phase()
        );

        let diagnostic = Diagnostic::new(
            DiagnosticCategory::UnsupportedCapability,
            DiagnosticCode::new("missing_remote_capability").unwrap(),
            "the remote executor does not advertise one required capability",
        )
        .at(DiagnosticPathSegment::Field("capabilities".into()))
        .at(DiagnosticPathSegment::Index(2));
        let classified = remote_diagnostic(diagnostic);

        assert_eq!(classified.category(), crate::ErrorCategory::Capability);
        assert_eq!(classified.code(), Some("missing_remote_capability"));
        assert_eq!(
            classified.path(),
            Some(&["capabilities".to_owned(), "[2]".to_owned()][..])
        );
        assert_eq!(classified.model_validation_phase(), None);
    }

    fn package() -> SchemaPackage<TestSchema> {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("remote.yaml").unwrap(),
            "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person:\n    owns: { name: { key: true } }\n",
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let resolved = resolve(
            &declared,
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
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
        let leak = |bytes: Vec<u8>| {
            Box::leak(String::from_utf8(bytes).unwrap().into_boxed_str()) as &'static str
        };
        SchemaPackage::new_with_declared(
            leak(to_canonical_json(projection.semantic_fingerprint()).unwrap()),
            leak(to_canonical_json(projection.projection_fingerprint()).unwrap()),
            leak(to_canonical_json(&projection).unwrap()),
            leak(encode_declared_schema(&declared).unwrap()),
        )
    }

    struct Transport {
        advertisement_contract: RemoteCapabilities,
        advertisement: Vec<u8>,
        capabilities: Arc<Mutex<usize>>,
        exchanges: Arc<Mutex<Vec<Vec<u8>>>>,
        failure: Option<Diagnostic>,
        signer: RemoteReplySigningKey,
    }

    impl RemoteQueryTransport for Transport {
        fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
            *self.capabilities.lock().unwrap() += 1;
            let bytes = self.advertisement.clone();
            Box::pin(async move { Ok(bytes) })
        }

        fn exchange<'a>(
            &'a self,
            request: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
            self.exchanges.lock().unwrap().push(request.to_vec());
            let response = (|| {
                let request = RemoteQueryRequestV2::decode(request).map_err(remote_diagnostic)?;
                request
                    .validate_advertisement(&self.advertisement_contract)
                    .map_err(remote_diagnostic)?;
                if let Some(diagnostic) = &self.failure {
                    return RemoteQueryFailureV2::bound(
                        request.nonce(),
                        &request.fingerprint().map_err(remote_diagnostic)?,
                        diagnostic,
                    )
                    .and_then(|failure| {
                        failure.encode_signed(
                            &self.advertisement_contract.fingerprint()?,
                            &self.signer,
                        )
                    })
                    .map_err(remote_diagnostic);
                }
                let plan = request.plan().map_err(remote_diagnostic)?;
                let root = BindingId::new(0).map_err(remote_diagnostic)?;
                let outcome = match request.result_kind() {
                    RemoteResultKindV2::DistinctCount => {
                        RemoteOutcomeV2::DistinctCount { root, value: 7 }
                    }
                    RemoteResultKindV2::DistinctExists => {
                        RemoteOutcomeV2::DistinctExists { root, value: true }
                    }
                    RemoteResultKindV2::HydratedRows => RemoteOutcomeV2::HydratedRows {
                        graph: HydrationGraphV2::new(vec![]).map_err(remote_diagnostic)?,
                        rows: vec![],
                    },
                    RemoteResultKindV2::HydratedPage => RemoteOutcomeV2::HydratedPage {
                        entries: vec![],
                        graph: HydrationGraphV2::new(vec![]).map_err(remote_diagnostic)?,
                        limit: 2,
                        offset: 0,
                        root,
                        total: Some(0),
                    },
                    _ => {
                        return Err(Error::Other {
                            message: "test transport received an unexpected terminal".into(),
                            source: None,
                        });
                    }
                };
                RemoteQueryResponseV2::new(
                    request.nonce(),
                    &plan,
                    &request.fingerprint().map_err(remote_diagnostic)?,
                    request.result_kind(),
                    outcome,
                )
                .and_then(|response| {
                    response
                        .encode_signed(&self.advertisement_contract.fingerprint()?, &self.signer)
                })
                .map_err(remote_diagnostic)
            })();
            Box::pin(async move { response })
        }
    }

    #[tokio::test]
    async fn remote_database_fetches_capabilities_once_and_exchanges_once_per_terminal() {
        let signer = RemoteReplySigningKey::from_secret_bytes([0x31; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let advertisement_contract = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("rust-client-test", "epoch-00000000001").unwrap(),
            signer.public_key(),
        );
        let advertisement = advertisement_contract.encode().unwrap();
        let capability_calls = Arc::new(Mutex::new(0));
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let transport = Transport {
            advertisement_contract,
            advertisement,
            capabilities: Arc::clone(&capability_calls),
            exchanges: Arc::clone(&exchanges),
            failure: None,
            signer,
        };
        let options = RemoteConnectionOptions::new(
            "rust-client-test",
            "typedb-3.12.1/v1",
            RemoteQueryLimits::new(10, 1 << 20, 10, 100, 100, 100),
            transport,
        );
        let remote = RemoteDatabase::connect(options)
            .await
            .unwrap()
            .with_schema(package())
            .unwrap();
        let mut session = remote.query().unwrap();
        let person = session.exact::<Person>().unwrap();
        let query = session.query(person).unwrap();

        assert_eq!(query.count().await.unwrap(), 7);
        assert!(query.exists().await.unwrap());
        assert!(
            query
                .rows(crate::RowsOptions::new(2))
                .await
                .unwrap()
                .is_empty()
        );
        let page = query
            .page_by(person, crate::PageOptions::new(2).include_total(true))
            .await
            .unwrap();
        assert!(page.items().is_empty());
        assert_eq!(page.total(), Some(0));
        let error = query
            .aggregate((crate::aggregate::count(),))
            .await
            .expect_err("native-only reductions fail before transport exchange");
        assert!(
            error
                .to_string()
                .contains("query_remote_v2_native_only_operation")
        );
        assert_eq!(*capability_calls.lock().unwrap(), 1);
        let requests = exchanges.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(
            std::str::from_utf8(&requests[0])
                .unwrap()
                .contains("\"format\":\"typebridge.query-remote-request/v2\"")
        );
    }

    #[tokio::test]
    async fn generated_remote_query_preserves_complete_authenticated_structured_diagnostic() {
        let signer = RemoteReplySigningKey::from_secret_bytes([0x42; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let advertisement_contract = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("rust-generated-acceptance", "epoch-00000000002").unwrap(),
            signer.public_key(),
        );
        let advertisement = advertisement_contract.encode().unwrap();
        let capability_calls = Arc::new(Mutex::new(0));
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let diagnostic = Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new("remote_application_failure").unwrap(),
            "the remote application rejected this query",
        )
        .with_path(DiagnosticPath::from_segments([
            DiagnosticPathSegment::Field("plan".into()),
            DiagnosticPathSegment::Index(0),
            DiagnosticPathSegment::Identifier("person".into()),
        ]))
        .with_detail("attempt", 7_i64)
        .with_detail("expected", vec!["person".to_owned(), "employee".to_owned()])
        .with_detail("retryable", false)
        .with_detail("subject", "person");
        let transport = Transport {
            advertisement_contract,
            advertisement,
            capabilities: Arc::clone(&capability_calls),
            exchanges: Arc::clone(&exchanges),
            failure: Some(diagnostic),
            signer,
        };
        let options = RemoteConnectionOptions::new(
            "rust-generated-acceptance",
            "typedb-3.12.1/v1",
            RemoteQueryLimits::new(10, 1 << 20, 10, 100, 100, 100),
            transport,
        );
        let remote = RemoteDatabase::connect(options)
            .await
            .unwrap()
            .with_schema(package())
            .unwrap();
        let mut session = remote.query().unwrap();
        let person = session.exact::<Person>().unwrap();
        let error = session
            .query(person)
            .unwrap()
            .one()
            .await
            .expect_err("generated query must return the authenticated application failure");

        assert_eq!(error.category(), crate::ErrorCategory::Remote);
        assert_eq!(error.code(), Some("remote_application_failure"));
        assert_eq!(
            error.message(),
            "the remote application rejected this query"
        );
        assert_eq!(
            error.path(),
            Some(&["plan".to_owned(), "[0]".to_owned(), "person".to_owned()][..])
        );
        assert_eq!(
            error.diagnostic_path(),
            Some(
                &[
                    crate::ErrorPathSegment::Field("plan".into()),
                    crate::ErrorPathSegment::Index(0),
                    crate::ErrorPathSegment::Identifier("person".into()),
                ][..]
            )
        );
        let details = error.details().expect("authenticated diagnostic details");
        assert_eq!(details.get("attempt"), Some(&crate::ErrorDetail::Long(7)));
        assert_eq!(
            details.get("expected"),
            Some(&crate::ErrorDetail::TextList(vec![
                "person".to_owned(),
                "employee".to_owned(),
            ]))
        );
        assert_eq!(
            details.get("retryable"),
            Some(&crate::ErrorDetail::Boolean(false))
        );
        assert_eq!(
            details.get("subject"),
            Some(&crate::ErrorDetail::Text("person".to_owned()))
        );
        assert_eq!(*capability_calls.lock().unwrap(), 1);
        assert_eq!(exchanges.lock().unwrap().len(), 1);
    }
}
