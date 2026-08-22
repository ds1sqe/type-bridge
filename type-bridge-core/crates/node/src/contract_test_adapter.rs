//! Feature-gated native adapters used only by cross-language contract tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::value::{CanonicalValue, Cardinality};
use type_bridge_core_lib::version::Version;
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
};
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, DatabaseConnectionAuthority, OrmError,
    ProviderRuntimeOwner, TxType,
};

use crate::NodeRustDatabase;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ContractFoundationProbe {
    capabilities: CapabilitySet,
    cardinality: Cardinality,
    fingerprint: Fingerprint,
    long: CanonicalValue,
    type_id: TypeId,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ProjectionRecordingResponse {
    Query(QueryResult),
    Failure {
        #[serde(rename = "providerFailure")]
        provider_failure: bool,
    },
}

/// Round-trip exact canonical foundation bytes through the N-API boundary.
///
/// This symbol exists only in builds that explicitly enable the private
/// `contract-test-adapter` feature.
#[napi(js_name = "__roundTripContractFoundation")]
pub fn round_trip_contract_foundation(input: Buffer) -> Result<Buffer> {
    let probe: ContractFoundationProbe =
        from_canonical_json(input.as_ref()).map_err(contract_error)?;
    if !matches!(probe.long, CanonicalValue::Long(_)) {
        return Err(Error::new(
            Status::InvalidArg,
            "contract foundation probe requires a tagged long",
        ));
    }
    to_canonical_json(&probe)
        .map(Buffer::from)
        .map_err(contract_error)
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectionRecordingState {
    pub(crate) opens: Vec<String>,
    pub(crate) queries: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) given_rows: Vec<GivenRowsSpec>,
    pub(crate) commits: usize,
    pub(crate) rollbacks: usize,
    pub(crate) closes: usize,
    #[serde(skip)]
    commit_failure: Option<CommitFailureCertainty>,
}

#[cfg(test)]
pub(crate) fn projection_recording_database_for_test(
    responses: Vec<ProjectionRecordingResponse>,
) -> (Arc<Database>, Arc<Mutex<ProjectionRecordingState>>) {
    let state = Arc::new(Mutex::new(ProjectionRecordingState::default()));
    let backend = ProjectionRecordingBackend {
        responses: Arc::new(Mutex::new(responses.into())),
        state: Arc::clone(&state),
    };
    (
        Arc::new(Database::with_backend(
            Box::new(backend),
            "projection-recording-test",
        )),
        state,
    )
}

struct ProjectionRecordingBackend {
    responses: Arc<Mutex<VecDeque<ProjectionRecordingResponse>>>,
    state: Arc<Mutex<ProjectionRecordingState>>,
}

impl DriverBackend for ProjectionRecordingBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        self.state
            .lock()
            .expect("recording state poisoned")
            .opens
            .push(
                match tx_type {
                    TxType::Read => "read",
                    TxType::Write => "write",
                    TxType::Schema => "schema",
                }
                .to_owned(),
            );
        let transaction = ProjectionRecordingTransaction {
            responses: Arc::clone(&self.responses),
            state: Arc::clone(&self.state),
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<Version> {
        Some(Version::new(3, 12, 1))
    }

    fn supports_given_rows(&self) -> bool {
        true
    }
}

struct ProjectionRecordingTransaction {
    responses: Arc<Mutex<VecDeque<ProjectionRecordingResponse>>>,
    state: Arc<Mutex<ProjectionRecordingState>>,
}

impl TransactionOps for ProjectionRecordingTransaction {
    fn supports_given_rows(&self) -> bool {
        true
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.query_canonical(typeql)
    }

    fn query_canonical(
        &mut self,
        typeql: &str,
    ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.state
            .lock()
            .expect("recording state poisoned")
            .queries
            .push(typeql.to_owned());
        let response = self
            .responses
            .lock()
            .expect("recording responses poisoned")
            .pop_front();
        Box::pin(async move {
            match response {
                Some(ProjectionRecordingResponse::Query(response)) => Ok(response),
                Some(ProjectionRecordingResponse::Failure {
                    provider_failure: true,
                }) => Err(OrmError::QueryExecution(
                    "contract recording provider failure".to_owned(),
                )),
                Some(ProjectionRecordingResponse::Failure {
                    provider_failure: false,
                })
                | None => Err(OrmError::QueryExecution(
                    "contract recording adapter received unexpected provider I/O".to_owned(),
                )),
            }
        })
    }

    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.state
            .lock()
            .expect("recording state poisoned")
            .given_rows
            .push(rows);
        self.query_canonical(typeql)
    }

    fn commit(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        Box::pin(async {
            Err(OrmError::QueryExecution(
                "contract recording adapter used the lossy commit path".to_owned(),
            ))
        })
    }

    fn commit_classified(
        &mut self,
    ) -> BoxFuture<'_, std::result::Result<(), ClassifiedCommitError>> {
        let failure = {
            let mut state = self.state.lock().expect("recording state poisoned");
            state.commits += 1;
            state.commit_failure
        };
        Box::pin(async move {
            match failure {
                Some(certainty) => Err(ClassifiedCommitError::Driver {
                    certainty,
                    message: "contract recording commit failed".to_owned(),
                }),
                None => Ok(()),
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        self.state
            .lock()
            .expect("recording state poisoned")
            .rollbacks += 1;
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        self.state.lock().expect("recording state poisoned").closes += 1;
        Box::pin(async { Ok(()) })
    }
}

/// Opaque shared provider identity for feature-gated origin tests.
#[napi(js_name = "__newProjectionRecordingAuthority")]
pub fn new_projection_recording_authority() -> External<DatabaseConnectionAuthority> {
    External::new(DatabaseConnectionAuthority::isolated())
}

/// One feature-gated database plus stable provider-I/O counters.
#[napi(js_name = "__ProjectionRecordingFixture")]
pub struct ProjectionRecordingFixture {
    database: Option<NodeRustDatabase>,
    state: Arc<Mutex<ProjectionRecordingState>>,
}

#[napi]
impl ProjectionRecordingFixture {
    #[napi(constructor)]
    pub fn new(
        responses_json: String,
        authority: Option<&External<DatabaseConnectionAuthority>>,
        commit_failure: Option<String>,
    ) -> Result<Self> {
        let responses: Vec<ProjectionRecordingResponse> = serde_json::from_str(&responses_json)
            .map_err(|error| {
                Error::new(Status::InvalidArg, format!("invalid responses: {error}"))
            })?;
        let commit_failure = match commit_failure.as_deref() {
            None => None,
            Some("definitely_aborted") => Some(CommitFailureCertainty::DefinitelyAborted),
            Some("unknown") => Some(CommitFailureCertainty::Unknown),
            Some(_) => {
                return Err(Error::new(
                    Status::InvalidArg,
                    "commit failure must be 'definitely_aborted' or 'unknown'",
                ));
            }
        };
        let state = Arc::new(Mutex::new(ProjectionRecordingState {
            commit_failure,
            ..ProjectionRecordingState::default()
        }));
        let backend = ProjectionRecordingBackend {
            responses: Arc::new(Mutex::new(responses.into())),
            state: Arc::clone(&state),
        };
        let database = match authority {
            Some(authority) => Database::with_backend_authority(
                Box::new(backend),
                "projection-recording",
                authority.as_ref().clone(),
            ),
            None => Database::with_backend(Box::new(backend), "projection-recording"),
        };
        let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
            Error::new(
                Status::GenericFailure,
                format!("failed to create recording runtime: {error}"),
            )
        })?;
        Ok(Self {
            database: Some(NodeRustDatabase {
                db: Arc::new(database),
                runtime,
                managed_scope_id: None,
            }),
            state,
        })
    }

    #[napi(js_name = "takeDatabase")]
    pub fn take_database(&mut self) -> Result<NodeRustDatabase> {
        self.database.take().ok_or_else(|| {
            Error::new(
                Status::InvalidArg,
                "recording fixture database was already taken",
            )
        })
    }

    #[napi(js_name = "countersJson")]
    pub fn counters_json(&self) -> Result<String> {
        serde_json::to_string(&*self.state.lock().expect("recording state poisoned"))
            .map_err(contract_json_error)
    }
}

fn contract_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    let payload = serde_json::to_string(&error)
        .unwrap_or_else(|_| format!(r#"{{"code":"{}","message":"{}"}}"#, error.code(), error));
    Error::new(Status::InvalidArg, payload)
}

fn contract_json_error(error: serde_json::Error) -> Error {
    Error::new(
        Status::GenericFailure,
        format!("adapter JSON failure: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use type_bridge_orm::GivenValue;

    use super::*;

    #[test]
    fn empty_recording_state_preserves_the_legacy_counter_shape() {
        assert_eq!(
            serde_json::to_value(ProjectionRecordingState::default()).unwrap(),
            json!({
                "opens": [],
                "queries": [],
                "commits": 0,
                "rollbacks": 0,
                "closes": 0,
            })
        );
    }

    #[test]
    fn recording_state_exposes_exact_given_row_order_only_when_present() {
        let state = ProjectionRecordingState {
            given_rows: vec![GivenRowsSpec {
                variables: vec!["ordinal".to_owned(), "target-iid".to_owned()],
                rows: vec![
                    vec![GivenValue::Integer(0), GivenValue::String("0xa".into())],
                    vec![GivenValue::Integer(1), GivenValue::String("0xb".into())],
                ],
            }],
            ..ProjectionRecordingState::default()
        };
        assert_eq!(
            serde_json::to_value(state).unwrap()["givenRows"],
            json!([{
                "variables": ["ordinal", "target-iid"],
                "rows": [
                    [{"Integer": 0}, {"String": "0xa"}],
                    [{"Integer": 1}, {"String": "0xb"}],
                ],
            }])
        );
    }

    #[test]
    fn recording_response_accepts_the_fixed_provider_failure_sentinel() {
        let responses: Vec<ProjectionRecordingResponse> =
            serde_json::from_value(json!([{"providerFailure": true}])).unwrap();
        assert!(matches!(
            responses.as_slice(),
            [ProjectionRecordingResponse::Failure {
                provider_failure: true,
            }]
        ));
    }
}
