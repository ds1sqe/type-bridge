#![deny(missing_docs)]
//! Authenticated one-exchange remote execution for generated queries.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, QueryExecutionDeadline,
    QueryExecutionResourceLimits, RemoteModelQueryV2Error, ValidatedMatchRequest,
    ValidatedMatchResult, lower_remote_query_diagnostic, prepare_remote_model_query_v2_with_budget,
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
    resources: QueryExecutionResourceLimits,
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
            resources: QueryExecutionResourceLimits::tightened(
                type_bridge_orm::MAX_QUERY_TIMEOUT_MILLISECONDS,
                max_items,
                max_bytes,
                max_graph_nodes,
                max_attribute_values,
                max_collection_members,
                max_role_players,
                type_bridge_orm::MAX_QUERY_STATEMENTS,
            ),
        }
    }

    /// Attach an optional executor deadline in milliseconds.
    #[must_use]
    pub const fn deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.resources.timeout_milliseconds =
            if deadline_ms < type_bridge_orm::MAX_QUERY_TIMEOUT_MILLISECONDS {
                deadline_ms
            } else {
                type_bridge_orm::MAX_QUERY_TIMEOUT_MILLISECONDS
            };
        self
    }
}

impl From<RemoteQueryLimits> for QueryExecutionResourceLimits {
    fn from(limits: RemoteQueryLimits) -> Self {
        limits.resources
    }
}

impl From<QueryExecutionResourceLimits> for RemoteQueryLimits {
    fn from(resources: QueryExecutionResourceLimits) -> Self {
        Self {
            resources: resources.effective(),
        }
    }
}

/// Connection-time authority, transport, and limit configuration.
pub struct RemoteConnectionOptions {
    scope: Option<String>,
    semantic_profile: Option<String>,
    resources: QueryExecutionResourceLimits,
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
        limits: impl Into<QueryExecutionResourceLimits>,
        transport: impl RemoteQueryTransport,
    ) -> Self {
        Self {
            scope: Some(scope.into()),
            semantic_profile: Some(semantic_profile.into()),
            resources: limits.into().effective(),
            transport: Arc::new(transport),
            advertisement: None,
        }
    }

    /// Construct normal generated-package options; schema scope and semantic
    /// profile are derived from [`SchemaPackage`] during binding.
    #[must_use]
    pub fn generated(
        limits: impl Into<QueryExecutionResourceLimits>,
        transport: impl RemoteQueryTransport,
    ) -> Self {
        Self {
            scope: None,
            semantic_profile: None,
            resources: limits.into().effective(),
            transport: Arc::new(transport),
            advertisement: None,
        }
    }
}

struct RemoteRuntime {
    advertisement: Vec<u8>,
    authority: Arc<QueryAuthority>,
    resources: QueryExecutionResourceLimits,
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
        let (installed, embedded_authority) = schema.verify_and_install_with_authority()?;
        let registry = Arc::new(installed.match_registry().map_err(Error::from_orm)?);
        let options = self.options.take().ok_or_else(|| Error::Other {
            message: "remote connection options are unavailable".into(),
            source: None,
        })?;
        let authority = if let Some(embedded) = embedded_authority {
            let embedded_scope = embedded.managed_scope().id().as_str();
            let embedded_profile = embedded.semantic_profile().id().as_str();
            if options
                .scope
                .as_deref()
                .is_some_and(|scope| scope != embedded_scope)
                || options
                    .semantic_profile
                    .as_deref()
                    .is_some_and(|profile| profile != embedded_profile)
            {
                return Err(Error::SchemaVerification {
                    message: "remote options disagree with generated schema authority".into(),
                    source: None,
                });
            }
            let declared =
                type_bridge_contract::schema::encode_declared_schema(embedded.declared_schema())
                    .map_err(|error| Error::SchemaVerification {
                        message: "verified generated authority cannot reconstruct its declaration"
                            .into(),
                        source: Some(Box::new(error)),
                    })?;
            QueryAuthority::from_declared_bytes(&declared, embedded_scope, embedded_profile)
                .map_err(remote_diagnostic)?
        } else {
            let declared =
                schema
                    .declared_schema_json()
                    .ok_or_else(|| Error::SchemaVerification {
                        message: "generated schema package omits remote declared-schema authority"
                            .into(),
                        source: None,
                    })?;
            let scope = options
                .scope
                .as_deref()
                .ok_or_else(|| Error::SchemaVerification {
                    message: "schema package has no embedded managed scope".into(),
                    source: None,
                })?;
            let semantic_profile =
                options
                    .semantic_profile
                    .as_deref()
                    .ok_or_else(|| Error::SchemaVerification {
                        message: "schema package has no embedded semantic profile".into(),
                        source: None,
                    })?;
            QueryAuthority::from_declared_bytes(declared.as_bytes(), scope, semantic_profile)
                .map_err(remote_diagnostic)?
        };
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
            resources: options.resources,
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
        let runtime = self.runtime.as_ref().ok_or_else(remote_not_bound)?;
        self.query_with_resources(runtime.resources, AnswerCancellation::default())
    }

    /// Start one remote query session with one common tighten-only resource
    /// policy and caller-owned cooperative cancellation signal.
    pub fn query_with_resources(
        &self,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<QuerySession<'_, S>> {
        let installed = self.installed.as_deref().ok_or_else(remote_not_bound)?;
        let registry = self.registry.as_ref().ok_or_else(remote_not_bound)?;
        let runtime = self.runtime.as_ref().ok_or_else(remote_not_bound)?;
        Ok(QuerySession::remote(
            installed,
            Arc::clone(registry),
            self,
            resources.constrained_by(runtime.resources),
            cancellation,
        ))
    }

    pub(crate) async fn execute_match(
        &self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
        deadline: QueryExecutionDeadline,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult)> {
        let runtime = self.runtime.as_ref().ok_or_else(remote_not_bound)?;
        check_remote_execution_budget(&cancellation, deadline)?;
        let pending = prepare_remote_model_query_v2_with_budget(
            &runtime.authority,
            registry,
            validated,
            &runtime.advertisement,
            resources.remote(),
            deadline,
            &cancellation,
        )
        .map_err(remote_model_input_error)?;
        check_remote_execution_budget(&cancellation, deadline)?;
        let request = pending.request_bytes().to_vec();
        let response = await_remote_exchange(
            runtime.transport.as_ref(),
            &request,
            &cancellation,
            deadline,
        )
        .await?;
        check_remote_execution_budget(&cancellation, deadline)?;
        let claimed = pending
            .claim_reply_with_cancellation(&cancellation)
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
            .decode_with_cancellation(&response, &cancellation)
            .map_err(remote_model_hydration_error)?;
        check_remote_execution_budget(&cancellation, deadline)?;
        Ok((request, result))
    }
}

async fn await_remote_exchange(
    transport: &dyn RemoteQueryTransport,
    request: &[u8],
    cancellation: &AnswerCancellation,
    deadline: QueryExecutionDeadline,
) -> Result<Vec<u8>> {
    check_remote_execution_budget(cancellation, deadline)?;
    let exchange = transport.exchange(request);
    tokio::pin!(exchange);
    let cancellation_wait = cancellation.cancelled();
    tokio::pin!(cancellation_wait);
    let timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.instant()));
    tokio::pin!(timeout);
    let result = tokio::select! {
        biased;
        result = &mut exchange => result,
        () = &mut cancellation_wait => Err(remote_cancelled()),
        () = &mut timeout => Err(remote_timeout()),
    };
    check_remote_execution_budget(cancellation, deadline)?;
    result
}

fn check_remote_execution_budget(
    cancellation: &AnswerCancellation,
    deadline: QueryExecutionDeadline,
) -> Result<()> {
    deadline
        .check(cancellation)
        .map_err(|error| Error::from_sdk_execution(error, crate::ModelValidationPhase::Input))
}

fn remote_cancelled() -> Error {
    Error::classified(
        crate::ErrorCategory::Cancelled,
        None,
        "provider_cancelled",
        Vec::new(),
        "query execution was cancelled",
        None,
    )
}

fn remote_timeout() -> Error {
    Error::classified(
        crate::ErrorCategory::ResourceLimit,
        None,
        "transaction_deadline_exceeded",
        Vec::new(),
        "query execution exceeded its timeout",
        None,
    )
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
    Error::from_sdk_execution(
        lower_remote_query_diagnostic(error),
        crate::ModelValidationPhase::Input,
    )
}

fn remote_model_input_error(error: RemoteModelQueryV2Error) -> Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(error) => Error::from_sdk_execution(
            lower_remote_query_diagnostic(error),
            crate::ModelValidationPhase::Input,
        ),
        RemoteModelQueryV2Error::Match(error) => {
            Error::from_match(error, crate::ModelValidationPhase::Input)
        }
    }
}

fn remote_model_hydration_error(error: RemoteModelQueryV2Error) -> Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(error) => Error::from_sdk_execution(
            lower_remote_query_diagnostic(error),
            crate::ModelValidationPhase::Hydration,
        ),
        RemoteModelQueryV2Error::Match(error) => {
            Error::from_match(error, crate::ModelValidationPhase::Hydration)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs::{self, OpenOptions};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

    use sha2::{Digest as _, Sha256};
    use tokio::sync::Notify;
    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPath, DiagnosticPathSegment,
    };
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::BindingId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::query_plan::{ModelQueryV2, query_plan_v2_capability_vocabulary};
    use type_bridge_contract::query_remote::RemoteExecutorBinding;
    use type_bridge_contract::query_remote_v2::{
        CAP_QUERY_REMOTE_STATEMENT_LIMIT, HydrationGraphV2, RemoteOutcomeV2, RemoteQueryFailureV2,
        RemoteQueryRequestV2, RemoteQueryResponseV2, RemoteReducedValueV2, RemoteReductionRowV2,
        RemoteResultKindV2, query_remote_v2_required_capabilities,
    };
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_orm::OrmError;
    use type_bridge_orm::match_request::CapabilitySet as MatchCapabilitySet;
    use type_bridge_orm::match_request::SessionHandle;
    use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
    };
    use type_bridge_schema::{
        ManagedDeltaContext, SchemaDocumentSet, build_schema_authority, encode_schema_authority,
        normalize_documents, project, resolve,
    };
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

    #[derive(Default)]
    struct DirectCancellationState {
        entered: Notify,
        opens: AtomicUsize,
        statements: AtomicUsize,
        closes: AtomicUsize,
    }

    struct DirectCancellationBackend {
        state: Arc<DirectCancellationState>,
    }

    impl DriverBackend for DirectCancellationBackend {
        fn match_capabilities(&self) -> MatchCapabilitySet {
            MatchCapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.opens.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(Box::new(DirectCancellationTransaction { state }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct DirectCancellationTransaction {
        state: Arc<DirectCancellationState>,
    }

    impl TransactionOps for DirectCancellationTransaction {
        fn query(
            &mut self,
            _typeql: &str,
        ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.statements.fetch_add(1, AtomicOrdering::SeqCst);
                state.entered.notify_one();
                std::future::pending().await
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.state.closes.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    fn direct_cancellation_database(
        state: Arc<DirectCancellationState>,
    ) -> crate::session::Database<TestSchema> {
        let generated = package();
        let installed = InstalledRuntimeProjection::from_verified_rust_json(
            generated.runtime_projection_json().as_bytes(),
            generated.semantic_fingerprint_json().as_bytes(),
            generated.projection_fingerprint_json().as_bytes(),
        )
        .expect("Rust cancellation proof installs the generated projection");
        crate::session::Database::from_test_parts(
            type_bridge_orm::Database::with_backend(
                Box::new(DirectCancellationBackend { state }),
                "rust-workforce-v2-proof",
            ),
            installed,
        )
    }

    async fn observe_workforce_v2_direct_cancellation() -> serde_json::Value {
        let pre_state = Arc::new(DirectCancellationState::default());
        let pre_database = direct_cancellation_database(Arc::clone(&pre_state));
        let pre_signal = AnswerCancellation::default();
        pre_signal.cancel();
        let mut pre_session = pre_database
            .query_with_resources(QueryExecutionResourceLimits::default(), pre_signal)
            .expect("Rust direct cancellation proof opens a generated session");
        let pre_person = pre_session
            .exact::<Person>()
            .expect("Rust direct cancellation proof binds a generated model");
        let pre_result = pre_session
            .query(pre_person)
            .expect("Rust direct cancellation proof authors a generated query")
            .count()
            .await;
        let pre_partial_result = pre_result.is_ok();
        let pre_error = pre_result.expect_err("pre-cancellation rejects the generated terminal");
        let pre_provider_calls = pre_state.opens.load(AtomicOrdering::SeqCst)
            + pre_state.statements.load(AtomicOrdering::SeqCst);
        assert_eq!(pre_error.category(), crate::ErrorCategory::Cancelled);
        assert_eq!(pre_error.code(), Some("provider_cancelled"));
        assert!(!pre_partial_result);
        assert_eq!(pre_provider_calls, 0);

        let in_flight_state = Arc::new(DirectCancellationState::default());
        let in_flight_database = direct_cancellation_database(Arc::clone(&in_flight_state));
        let in_flight_signal = AnswerCancellation::default();
        let requester_state = Arc::clone(&in_flight_state);
        let requester_signal = in_flight_signal.clone();
        let requester = tokio::spawn(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                requester_state.entered.notified(),
            )
            .await
            .expect("direct provider await is reached before cancellation");
            requester_signal.cancel();
            true
        });
        let mut in_flight_session = in_flight_database
            .query_with_resources(QueryExecutionResourceLimits::default(), in_flight_signal)
            .expect("Rust in-flight cancellation proof opens a generated session");
        let in_flight_person = in_flight_session
            .exact::<Person>()
            .expect("Rust in-flight cancellation proof binds a generated model");
        let in_flight_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            in_flight_session
                .query(in_flight_person)
                .expect("Rust in-flight cancellation proof authors a generated query")
                .count(),
        )
        .await
        .expect("cancellation wakes the pending provider await");
        let in_flight_partial_result = in_flight_result.is_ok();
        let in_flight_error =
            in_flight_result.expect_err("in-flight cancellation rejects the generated terminal");
        let provider_await_woken = requester
            .await
            .expect("direct cancellation requester completes")
            && in_flight_state.statements.load(AtomicOrdering::SeqCst) == 1;
        assert_eq!(in_flight_error.category(), crate::ErrorCategory::Cancelled);
        assert_eq!(in_flight_error.code(), Some("provider_cancelled"));
        assert!(!in_flight_partial_result);
        assert!(provider_await_woken);
        assert_eq!(in_flight_state.opens.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(in_flight_state.closes.load(AtomicOrdering::SeqCst), 1);

        serde_json::json!({
            "in_flight": {
                "category": in_flight_error.category().as_str(),
                "code": in_flight_error.code().expect("cancelled errors carry a stable code"),
                "partial_result": in_flight_partial_result,
                "provider_await_woken": provider_await_woken,
            },
            "pre_dispatch": {
                "category": pre_error.category().as_str(),
                "code": pre_error.code().expect("cancelled errors carry a stable code"),
                "partial_result": pre_partial_result,
                "provider_calls": pre_provider_calls,
            },
        })
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

    #[test]
    fn remote_model_resource_limits_use_released_generated_error_codes() {
        let internal = Diagnostic::new(
            DiagnosticCategory::ResourceLimit,
            DiagnosticCode::new("query_v2_model_role_player_limit").unwrap(),
            "internal model execution detail",
        );
        let error = remote_model_hydration_error(RemoteModelQueryV2Error::Diagnostic(internal));

        assert_eq!(error.category(), crate::ErrorCategory::ResourceLimit);
        assert_eq!(error.code(), Some("hydrated_role_player_limit"));
        assert_eq!(error.path(), Some(&["provider_evidence".to_owned()][..]));
        assert_eq!(
            error.diagnostic_path(),
            Some(
                &[crate::ErrorPathSegment::Query(
                    crate::QueryDiagnosticPathKind::ProviderEvidence,
                )][..]
            )
        );
        assert_eq!(error.model_validation_phase(), None);
        assert_eq!(
            error
                .details()
                .and_then(|details| details.get("query_category")),
            Some(&crate::ErrorDetail::QueryCategory(
                crate::QueryDiagnosticCategory::ResourceLimit,
            ))
        );
    }

    fn package() -> SchemaPackage<TestSchema> {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("remote.yaml").unwrap(),
            "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person:\n    owns: { name: { key: true } }\n",
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let authority = build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &ManagedDeltaContext::new(
                ManagedScopeId::new("rust-client-test").unwrap(),
                profile,
                CapabilitySet::new(),
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
        let leak = |bytes: Vec<u8>| {
            Box::leak(String::from_utf8(bytes).unwrap().into_boxed_str()) as &'static str
        };
        SchemaPackage::new_with_authority(
            leak(to_canonical_json(projection.semantic_fingerprint()).unwrap()),
            leak(to_canonical_json(projection.projection_fingerprint()).unwrap()),
            leak(to_canonical_json(&projection).unwrap()),
            leak(encode_schema_authority(&authority)),
            leak(encode_declared_schema(&declared).unwrap()),
            "rust-client-test",
            "typedb-3.12.1/v1",
        )
    }

    fn released_declared_package() -> SchemaPackage<TestSchema> {
        let generated = package();
        SchemaPackage::new_with_declared(
            generated.semantic_fingerprint_json(),
            generated.projection_fingerprint_json(),
            generated.runtime_projection_json(),
            generated
                .declared_schema_json()
                .expect("test package carries a declaration"),
        )
    }

    struct UnusedTransport;

    impl RemoteQueryTransport for UnusedTransport {
        fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
            Box::pin(async { panic!("compatibility test performs no transport I/O") })
        }

        fn exchange<'a>(
            &'a self,
            _request: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
            Box::pin(async { panic!("compatibility test performs no transport I/O") })
        }
    }

    fn unconnected_remote(mut options: RemoteConnectionOptions) -> RemoteDatabase<Unbound> {
        options.advertisement = Some(Vec::new());
        RemoteDatabase {
            options: Some(options),
            runtime: None,
            installed: None,
            registry: None,
            marker: PhantomData,
        }
    }

    #[test]
    fn generated_authority_accepts_matching_legacy_options_and_rejects_overrides() {
        let limits = || RemoteQueryLimits::new(10, 1 << 20, 10, 100, 100, 100);
        let matching = RemoteConnectionOptions::new(
            "rust-client-test",
            "typedb-3.12.1/v1",
            limits(),
            UnusedTransport,
        );
        unconnected_remote(matching)
            .with_schema(package())
            .expect("matching 2.0.1-style options remain compatible");

        for mismatched in [
            RemoteConnectionOptions::new(
                "other-scope",
                "typedb-3.12.1/v1",
                limits(),
                UnusedTransport,
            ),
            RemoteConnectionOptions::new(
                "rust-client-test",
                "typedb-3.11.5/v1",
                limits(),
                UnusedTransport,
            ),
        ] {
            let error = unconnected_remote(mismatched)
                .with_schema(package())
                .expect_err("caller strings cannot override generated authority");
            assert!(
                error
                    .to_string()
                    .contains("disagree with generated schema authority"),
                "{error}"
            );
        }
    }

    #[test]
    fn released_declared_package_retains_explicit_remote_options_compatibility() {
        let limits = || RemoteQueryLimits::new(10, 1 << 20, 10, 100, 100, 100);
        let options = RemoteConnectionOptions::new(
            "rust-client-test",
            "typedb-3.12.1/v1",
            limits(),
            UnusedTransport,
        );
        unconnected_remote(options)
            .with_schema(released_declared_package())
            .expect("2.0.1 generated package and explicit options remain compatible");

        let generated_options = RemoteConnectionOptions::generated(limits(), UnusedTransport);
        let error = unconnected_remote(generated_options)
            .with_schema(released_declared_package())
            .expect_err("detached 2.0.1 package cannot invent embedded deployment authority");
        assert!(error.to_string().contains("no embedded managed scope"));
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
                    RemoteResultKindV2::ModelReduction => {
                        let Some(ModelQueryV2::Reduction {
                            root,
                            group,
                            reducers,
                            ..
                        }) = plan
                            .v2_compatibility()
                            .and_then(|compatibility| compatibility.model_query())
                        else {
                            return Err(Error::Other {
                                message: "test reduction lacks its model contract".into(),
                                source: None,
                            });
                        };
                        RemoteOutcomeV2::ModelReduction {
                            graph: HydrationGraphV2::new(vec![]).map_err(remote_diagnostic)?,
                            root: *root,
                            group: group.clone(),
                            reducers: reducers.clone(),
                            rows: vec![RemoteReductionRowV2::new(
                                None,
                                vec![RemoteReducedValueV2::Count { value: 7 }],
                            )],
                        }
                    }
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

    struct CancelBeforeDecodeTransport {
        inner: Transport,
        cancellation: AnswerCancellation,
        response_completed: Arc<AtomicBool>,
    }

    impl RemoteQueryTransport for CancelBeforeDecodeTransport {
        fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
            self.inner.capabilities()
        }

        fn exchange<'a>(
            &'a self,
            request: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
            Box::pin(async move {
                let response = self.inner.exchange(request).await?;
                self.response_completed.store(true, AtomicOrdering::SeqCst);
                self.cancellation.cancel();
                Ok(response)
            })
        }
    }

    struct ExchangeDropProbe(Arc<AtomicBool>);

    impl Drop for ExchangeDropProbe {
        fn drop(&mut self) {
            self.0.store(true, AtomicOrdering::SeqCst);
        }
    }

    struct CallerAbortTransport {
        advertisement: Vec<u8>,
        exchanges: Arc<AtomicUsize>,
        entered: Arc<Notify>,
        exchange_dropped: Arc<AtomicBool>,
    }

    impl RemoteQueryTransport for CallerAbortTransport {
        fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
            let advertisement = self.advertisement.clone();
            Box::pin(async move { Ok(advertisement) })
        }

        fn exchange<'a>(
            &'a self,
            _request: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
            self.exchanges.fetch_add(1, AtomicOrdering::SeqCst);
            self.entered.notify_one();
            let probe = ExchangeDropProbe(Arc::clone(&self.exchange_dropped));
            Box::pin(async move {
                let _probe = probe;
                std::future::pending().await
            })
        }
    }

    fn workforce_v2_transport(seed: u8) -> (Transport, Arc<Mutex<Vec<Vec<u8>>>>) {
        let signer = RemoteReplySigningKey::from_secret_bytes([seed; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_STATEMENT_LIMIT).unwrap());
        let advertisement_contract = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("rust-client-test", "epoch-00000000003").unwrap(),
            signer.public_key(),
        );
        let advertisement = advertisement_contract.encode().unwrap();
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        (
            Transport {
                advertisement_contract,
                advertisement,
                capabilities: Arc::new(Mutex::new(0)),
                exchanges: Arc::clone(&exchanges),
                failure: None,
                signer,
            },
            exchanges,
        )
    }

    async fn observe_workforce_v2_remote_cancellation() -> serde_json::Value {
        let (pre_transport, pre_exchanges) = workforce_v2_transport(0x71);
        let remote = RemoteDatabase::connect(RemoteConnectionOptions::generated(
            QueryExecutionResourceLimits::default(),
            pre_transport,
        ))
        .await
        .expect("Rust remote cancellation proof connects")
        .with_schema(package())
        .expect("Rust remote cancellation proof binds generated schema authority");
        let pre_signal = AnswerCancellation::default();
        pre_signal.cancel();
        let mut pre_session = remote
            .query_with_resources(QueryExecutionResourceLimits::default(), pre_signal)
            .expect("Rust pre-cancelled remote session opens");
        let pre_person = pre_session
            .exact::<Person>()
            .expect("Rust pre-cancelled remote model binds");
        let pre_result = pre_session
            .query(pre_person)
            .expect("Rust pre-cancelled remote query authors")
            .count()
            .await;
        let pre_partial_result = pre_result.is_ok();
        let pre_error = pre_result.expect_err("pre-cancellation rejects before remote exchange");
        let pre_exchange_count = pre_exchanges.lock().unwrap().len();
        assert_eq!(pre_error.category(), crate::ErrorCategory::Cancelled);
        assert_eq!(pre_error.code(), Some("provider_cancelled"));
        assert_eq!(pre_exchange_count, 0);
        assert!(!pre_partial_result);

        let decode_signal = AnswerCancellation::default();
        let response_completed = Arc::new(AtomicBool::new(false));
        let (decode_inner, decode_exchanges) = workforce_v2_transport(0x72);
        let decode_remote = RemoteDatabase::connect(RemoteConnectionOptions::generated(
            QueryExecutionResourceLimits::default(),
            CancelBeforeDecodeTransport {
                inner: decode_inner,
                cancellation: decode_signal.clone(),
                response_completed: Arc::clone(&response_completed),
            },
        ))
        .await
        .expect("Rust decode-cancel remote connects")
        .with_schema(package())
        .expect("Rust decode-cancel remote binds generated schema authority");
        let mut decode_session = decode_remote
            .query_with_resources(QueryExecutionResourceLimits::default(), decode_signal)
            .expect("Rust decode-cancel session opens");
        let decode_person = decode_session
            .exact::<Person>()
            .expect("Rust decode-cancel model binds");
        let decode_result = decode_session
            .query(decode_person)
            .expect("Rust decode-cancel query authors")
            .count()
            .await;
        let decode_partial_result = decode_result.is_ok();
        let decode_error = decode_result.expect_err("cancellation rejects before reply decode");
        let decode_exchange_count = decode_exchanges.lock().unwrap().len();
        assert_eq!(decode_error.category(), crate::ErrorCategory::Cancelled);
        assert_eq!(decode_error.code(), Some("provider_cancelled"));
        assert_eq!(decode_exchange_count, 1);
        assert!(!decode_partial_result);

        let (abort_contract_transport, _) = workforce_v2_transport(0x73);
        let abort_exchanges = Arc::new(AtomicUsize::new(0));
        let abort_entered = Arc::new(Notify::new());
        let abort_dropped = Arc::new(AtomicBool::new(false));
        let abort_transport = CallerAbortTransport {
            advertisement: abort_contract_transport.advertisement,
            exchanges: Arc::clone(&abort_exchanges),
            entered: Arc::clone(&abort_entered),
            exchange_dropped: Arc::clone(&abort_dropped),
        };
        let abort_remote = RemoteDatabase::connect(RemoteConnectionOptions::generated(
            QueryExecutionResourceLimits::default(),
            abort_transport,
        ))
        .await
        .expect("Rust caller-abort remote connects")
        .with_schema(package())
        .expect("Rust caller-abort remote binds generated schema authority");
        let abort_signal = AnswerCancellation::default();
        let requester_signal = abort_signal.clone();
        let requester_entered = Arc::clone(&abort_entered);
        let requester = tokio::spawn(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                requester_entered.notified(),
            )
            .await
            .expect("caller transport exchange starts before cancellation");
            requester_signal.cancel();
        });
        let mut abort_session = abort_remote
            .query_with_resources(QueryExecutionResourceLimits::default(), abort_signal)
            .expect("Rust caller-abort session opens");
        let abort_person = abort_session
            .exact::<Person>()
            .expect("Rust caller-abort model binds");
        let abort_error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            abort_session
                .query(abort_person)
                .expect("Rust caller-abort query authors")
                .count(),
        )
        .await
        .expect("caller cancellation wakes the remote exchange")
        .expect_err("caller cancellation rejects the remote terminal");
        requester
            .await
            .expect("caller transport cancellation requester completes");
        let caller_transport_abort_supported = abort_error.category()
            == crate::ErrorCategory::Cancelled
            && abort_error.code() == Some("provider_cancelled")
            && abort_exchanges.load(AtomicOrdering::SeqCst) == 1
            && abort_dropped.load(AtomicOrdering::SeqCst);
        assert!(caller_transport_abort_supported);

        serde_json::json!({
            "before_exchange": {
                "category": pre_error.category().as_str(),
                "code": pre_error.code().expect("cancelled errors carry a stable code"),
                "exchange_count": pre_exchange_count,
                "partial_result": pre_partial_result,
            },
            "during_decode": {
                "category": decode_error.category().as_str(),
                "code": decode_error.code().expect("cancelled errors carry a stable code"),
                "exchange_count": decode_exchange_count,
                "partial_result": decode_partial_result,
            },
            "caller_transport_abort_supported": caller_transport_abort_supported,
            "server_exchange_cancelled_after_send": !response_completed.load(AtomicOrdering::SeqCst),
        })
    }

    fn workforce_v2_query_category(error: &crate::Error) -> &'static str {
        error
            .details()
            .and_then(|details| {
                details.values().find_map(|detail| match detail {
                    crate::ErrorDetail::QueryCategory(
                        crate::QueryDiagnosticCategory::ResultDecode,
                    ) => Some("result_decode"),
                    _ => None,
                })
            })
            .expect("authenticated remote diagnostic carries result_decode")
    }

    fn workforce_v2_remote_diagnostic_observation(
        error: &crate::Error,
        claim_consumed: bool,
    ) -> serde_json::Value {
        let path = error
            .diagnostic_path()
            .expect("authenticated remote diagnostic carries typed path")
            .iter()
            .map(|segment| match segment {
                crate::ErrorPathSegment::ContractField(value) => {
                    serde_json::json!({"kind": "contract_field", "value": value})
                }
                crate::ErrorPathSegment::Index(value) => {
                    serde_json::json!({"kind": "index", "value": value})
                }
                crate::ErrorPathSegment::ContractIdentity(value) => {
                    serde_json::json!({"kind": "contract_identity", "value": value})
                }
                other => panic!("unexpected authenticated proof path segment: {other:?}"),
            })
            .collect::<Vec<_>>();
        let mut details = serde_json::Map::new();
        for (name, detail) in error
            .details()
            .expect("authenticated remote diagnostic carries typed details")
        {
            let value = match detail {
                crate::ErrorDetail::Long(value) => {
                    serde_json::json!({"kind": "signed", "value": value.to_string()})
                }
                crate::ErrorDetail::Boolean(value) => {
                    serde_json::json!({"kind": "boolean", "value": value})
                }
                crate::ErrorDetail::QueryIdentity(value) => {
                    serde_json::json!({"kind": "query_identity", "value": value})
                }
                crate::ErrorDetail::QueryIdentityList(values) => {
                    serde_json::json!({"kind": "query_identity_list", "value": values})
                }
                crate::ErrorDetail::QueryCategory(_) => continue,
                other => panic!("unexpected authenticated proof detail: {other:?}"),
            };
            assert!(details.insert(name.clone(), value).is_none());
        }
        let visible = format!("{}{:?}{:?}", error.message(), path, details);
        let redacted = !visible.contains("provider-secret") && !visible.contains("must-not-cross");
        serde_json::json!({
            "category": match (error.category(), workforce_v2_query_category(error)) {
                (crate::ErrorCategory::ModelValidation, "result_decode") => "integrity",
                (category, _) => category.as_str(),
            },
            "query_category": workforce_v2_query_category(error),
            "code": error.code().expect("authenticated remote diagnostic carries a stable code"),
            "message": error.message(),
            "path": path,
            "details": details,
            "redacted": redacted,
            "claim_consumed": claim_consumed,
        })
    }

    async fn observe_workforce_v2_remote_structured_diagnostic() -> serde_json::Value {
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
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let failure = Diagnostic::new(
            DiagnosticCategory::Integrity,
            DiagnosticCode::new("remote_application_failure").unwrap(),
            "provider-secret remote query literal and endpoint",
        )
        .with_path(DiagnosticPath::from_segments([
            DiagnosticPathSegment::Field("plan".into()),
            DiagnosticPathSegment::Index(2),
            DiagnosticPathSegment::Identifier("person".into()),
        ]))
        .with_detail("attempt", -7_i64)
        .with_detail("expected", vec!["person".to_owned(), "employee".to_owned()])
        .with_detail("retryable", false)
        .with_detail("subject", "person")
        .with_detail("provider_secret", "must-not-cross");
        let transport = Transport {
            advertisement_contract,
            advertisement,
            capabilities: Arc::new(Mutex::new(0)),
            exchanges: Arc::clone(&exchanges),
            failure: Some(failure),
            signer,
        };
        let remote = RemoteDatabase::connect(RemoteConnectionOptions::generated(
            RemoteQueryLimits::new(10, 1 << 20, 10, 100, 100, 100),
            transport,
        ))
        .await
        .expect("Rust structured-diagnostic proof connects")
        .with_schema(package())
        .expect("Rust structured-diagnostic proof binds generated schema authority");
        let mut session = remote
            .query()
            .expect("Rust structured-diagnostic proof opens a generated session");
        let person = session
            .exact::<Person>()
            .expect("Rust structured-diagnostic proof binds a generated model");
        let query = session
            .query(person)
            .expect("Rust structured-diagnostic proof authors a generated query");
        let error = query
            .one()
            .await
            .expect_err("signed application failure reaches the generated query facade");
        let exchange_count = exchanges.lock().unwrap().len();
        let claim_consumed = exchange_count == 1
            && error.code() == Some("remote_application_failure")
            && error.details().is_some();
        assert_eq!(exchange_count, 1);
        assert!(claim_consumed);
        let observation = workforce_v2_remote_diagnostic_observation(&error, claim_consumed);
        assert_eq!(observation["redacted"], true);
        observation
    }

    fn workforce_v2_proof_source(root: &Path, relative: &str) -> serde_json::Value {
        let path = root.join(relative);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("proof source {} is readable: {error}", path.display()));
        serde_json::json!({
            "path": relative,
            "sha256": format!("{:x}", Sha256::digest(bytes)),
        })
    }

    fn requested_workforce_v2_proof_fragment() -> Option<(PathBuf, String)> {
        let destination = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT");
        let run_nonce = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE");
        assert_eq!(
            destination.is_some(),
            run_nonce.is_some(),
            "Rust proof fragment output and run nonce must be configured together",
        );
        let destination = PathBuf::from(destination?);
        assert!(
            destination.is_absolute(),
            "Rust proof destination must be absolute"
        );
        let parent = destination
            .parent()
            .expect("Rust proof destination has a parent");
        let parent_metadata =
            fs::symlink_metadata(parent).expect("Rust proof destination parent exists");
        assert!(
            parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
            "Rust proof destination parent must be a non-symlink directory"
        );
        assert!(
            matches!(fs::symlink_metadata(&destination), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "Rust proof destination must not already exist"
        );
        let run_nonce = run_nonce
            .expect("Rust proof run nonce exists")
            .into_string()
            .expect("Rust proof run nonce must be UTF-8");
        assert!(
            run_nonce.len() == 64
                && run_nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "Rust proof run nonce must be 64 lowercase hexadecimal digits"
        );
        Some((destination, run_nonce))
    }

    fn publish_workforce_v2_rust_proof_fragment(
        destination: &Path,
        run_nonce: &str,
        results: Vec<serde_json::Value>,
    ) {
        let core = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("Rust crate lives beneath type-bridge-core");
        let repository = core
            .parent()
            .expect("type-bridge-core has a repository parent");
        let sources = ["type-bridge-core/crates/rust/src/remote.rs"];
        let fragment = serde_json::json!({
            "format": "typebridge.workforce-v2-proof-fragment/v1",
            "binding": "rust",
            "semantic_profile": "typedb-3.12.1/v1",
            "run_nonce": run_nonce,
            "contract": {
                "allowlist": workforce_v2_proof_source(repository, "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-allowlist-v1.json"),
                "journey": workforce_v2_proof_source(repository, "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json"),
                "proof_schema": workforce_v2_proof_source(repository, "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-schema-v1.json"),
            },
            "producer": {
                "id": "type-bridge-rust.generated-query-proof",
                "sources": sources
                    .iter()
                    .map(|source| workforce_v2_proof_source(repository, source))
                    .collect::<Vec<_>>(),
            },
            "results": results,
        });
        let mut bytes =
            to_canonical_json(&fragment).expect("Rust workforce-v2 proof fragment canonicalizes");
        bytes.push(b'\n');
        assert!(bytes.len() <= 64 * 1024);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .expect("Rust workforce-v2 proof fragment is created once");
        output
            .write_all(&bytes)
            .expect("Rust workforce-v2 proof fragment is written completely");
        output
            .sync_all()
            .expect("Rust workforce-v2 proof fragment is durable");
    }

    #[tokio::test]
    async fn remote_database_fetches_capabilities_once_and_exchanges_once_per_terminal() {
        let signer = RemoteReplySigningKey::from_secret_bytes([0x31; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_STATEMENT_LIMIT).unwrap());
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
        let connection_resources =
            QueryExecutionResourceLimits::tightened(20_000, 10, 1_000_000, 90, 80, 70, 60, 2);
        let options = RemoteConnectionOptions::generated(connection_resources, transport);
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
        let reduction = query
            .aggregate((crate::aggregate::count(),))
            .await
            .expect("typed reductions use the same authenticated exchange");
        assert_eq!(reduction, (7,));
        assert_eq!(*capability_calls.lock().unwrap(), 1);

        let zero_timeout = QueryExecutionResourceLimits {
            timeout_milliseconds: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let mut timed_session = remote
            .query_with_resources(zero_timeout, AnswerCancellation::default())
            .unwrap();
        let timed_person = timed_session.exact::<Person>().unwrap();
        let timed_error = timed_session
            .query(timed_person)
            .unwrap()
            .count()
            .await
            .expect_err("zero timeout must reject before caller transport");
        assert_eq!(timed_error.category(), crate::ErrorCategory::ResourceLimit);
        assert_eq!(timed_error.code(), Some("transaction_deadline_exceeded"));

        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let mut cancelled_session = remote
            .query_with_resources(QueryExecutionResourceLimits::default(), cancellation)
            .unwrap();
        let cancelled_person = cancelled_session.exact::<Person>().unwrap();
        let cancelled_error = cancelled_session
            .query(cancelled_person)
            .unwrap()
            .count()
            .await
            .expect_err("pre-cancellation must reject before caller transport");
        assert_eq!(cancelled_error.category(), crate::ErrorCategory::Cancelled);
        assert_eq!(cancelled_error.code(), Some("provider_cancelled"));

        let terminal_resources =
            QueryExecutionResourceLimits::tightened(10_000, 20, 900_000, 100, 70, 80, 50, 1);
        let mut tightened_session = remote
            .query_with_resources(terminal_resources, AnswerCancellation::default())
            .unwrap();
        let tightened_person = tightened_session.exact::<Person>().unwrap();
        assert_eq!(
            tightened_session
                .query(tightened_person)
                .unwrap()
                .count()
                .await
                .unwrap(),
            7,
        );

        let requests = exchanges.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert!(
            std::str::from_utf8(&requests[0])
                .unwrap()
                .contains("\"format\":\"typebridge.query-remote-request/v2\"")
        );
        let request = RemoteQueryRequestV2::decode(&requests[5]).unwrap();
        let limits = request.limits();
        assert!(
            limits
                .deadline_ms
                .is_some_and(|deadline| deadline <= 10_000),
            "the wire carries the remaining portion of the tighter terminal timeout",
        );
        assert_eq!(limits.max_items, 10);
        assert_eq!(limits.max_bytes, 900_000);
        assert_eq!(limits.max_graph_nodes, 90);
        assert_eq!(limits.max_attribute_values, 70);
        assert_eq!(limits.max_collection_members, 70);
        assert_eq!(limits.max_role_players, 50);
        assert_eq!(limits.max_statements, 1);
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
            DiagnosticCategory::Integrity,
            DiagnosticCode::new("remote_application_failure").unwrap(),
            "the remote application rejected this query",
        )
        .with_path(DiagnosticPath::from_segments([
            DiagnosticPathSegment::Field("plan".into()),
            DiagnosticPathSegment::Index(2),
            DiagnosticPathSegment::Identifier("person".into()),
        ]))
        .with_detail("attempt", -7_i64)
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
        let options = RemoteConnectionOptions::generated(
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

        assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
        assert_eq!(error.code(), Some("remote_application_failure"));
        assert_eq!(
            error.message(),
            "Typed query evidence does not match the validated request invocation"
        );
        assert_eq!(
            error.path(),
            Some(&["plan".to_owned(), "[2]".to_owned(), "person".to_owned()][..])
        );
        assert_eq!(
            error.diagnostic_path(),
            Some(
                &[
                    crate::ErrorPathSegment::ContractField("plan".into()),
                    crate::ErrorPathSegment::Index(2),
                    crate::ErrorPathSegment::ContractIdentity("person".into()),
                ][..]
            )
        );
        let details = error.details().expect("authenticated diagnostic details");
        assert_eq!(details.get("attempt"), Some(&crate::ErrorDetail::Long(-7)));
        assert_eq!(
            details.get("expected"),
            Some(&crate::ErrorDetail::QueryIdentityList(vec![
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
            Some(&crate::ErrorDetail::QueryIdentity("person".to_owned()))
        );
        assert_eq!(
            details.get("query_category"),
            Some(&crate::ErrorDetail::QueryCategory(
                crate::QueryDiagnosticCategory::ResultDecode
            ))
        );
        assert_eq!(*capability_calls.lock().unwrap(), 1);
        assert_eq!(exchanges.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn workforce_v2_rust_deterministic_proof_fragment() {
        let test_id = "remote::tests::workforce_v2_rust_deterministic_proof_fragment";
        let results = vec![
            serde_json::json!({
                "observation_ref": "cancellation_direct",
                "proof_kind": "direct_runtime",
                "test_id": test_id,
                "outcome": "passed",
                "observation": observe_workforce_v2_direct_cancellation().await,
            }),
            serde_json::json!({
                "observation_ref": "cancellation_remote",
                "proof_kind": "remote_runtime",
                "test_id": test_id,
                "outcome": "passed",
                "observation": observe_workforce_v2_remote_cancellation().await,
            }),
            serde_json::json!({
                "observation_ref": "remote_structured_diagnostic",
                "proof_kind": "diagnostic",
                "test_id": test_id,
                "outcome": "passed",
                "observation": observe_workforce_v2_remote_structured_diagnostic().await,
            }),
        ];
        if let Some((destination, run_nonce)) = requested_workforce_v2_proof_fragment() {
            publish_workforce_v2_rust_proof_fragment(&destination, &run_nonce, results);
        }
    }
}
