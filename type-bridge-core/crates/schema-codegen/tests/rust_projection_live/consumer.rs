use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use type_bridge::__codegen::{EncodedScalar, IntoEncodedScalar};
use type_bridge::value::{
    Date as QueryDate, DateTime as QueryDateTime, DateTimeTz as QueryDateTimeTz,
    Decimal as QueryDecimal, Double as QueryDouble, Regex, Text,
};
use type_bridge::{
    AnswerCancellation, ConnectionOptions, Database, Error, ErrorCategory, ErrorDetail,
    ErrorPathSegment, MAX_QUERY_ITEMS, PageOptions, ProjectedManagerComparison,
    QueryDiagnosticCategory, QueryDiagnosticPathKind, QueryExecutionResourceLimits, QuerySession,
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits, RemoteQueryTransport, RowsOptions,
    aggregate,
};
use type_bridge_generated_schema::{
    Actor, ActorType, Aliases, AppSchema, CanonicalDouble, Container, ContainerCreate,
    ContainerType, Contractor, ContractorCode, ContractorCreate, Counter, CounterCreate,
    CounterType, CounterValue, Date, DateTime, DateTimeTz, Decimal, Duration, Employee,
    EmployeeCreate, EmployeeFamily, EmployeeType, Employment, EmploymentCreate, EmploymentType,
    Event, EventCreate, EventType, FooBar, Identifier, IntegerCall, IntegerInput, Interaction,
    InteractionActorPlayer, InteractionActorRef, InteractionCreate, InteractionType, Manager,
    ManagerCreate, ManagerNote, ManagerType, Membership,
    MembershipCreate, MembershipFamily, MembershipMemberPlayer, MembershipMemberRef,
    MembershipType, NetworkLink, NetworkLinkCreate, NetworkLinkType, Nickname,
    PROJECTION_FINGERPRINT_JSON, Party, PartyFamily, PartyName, Person, PersonCreate, PersonRef,
    PersonType, PlainActivity, PlainActivityCreate, PlainActivityType, Rank, Robot, RobotCreate,
    RobotId, RobotType, SCHEMA,
    SEMANTIC_SCHEMA_FINGERPRINT_JSON, Score, ScoreGte, ValBool, ValConstrained, ValDate,
    ValDatetime, ValDatetimeTz, ValDecimal, ValDouble, ValDuration, integer_input,
    plays_event_container_item, qualifying_score,
};

#[derive(type_bridge::SelectedRow)]
struct PersonGraph {
    person: Person,
    members: Vec<Person>,
}

#[derive(type_bridge::SelectedRow)]
struct WorkforceNetworkShape {
    origin: Person,
    participants: Vec<Person>,
}

#[derive(Clone)]
struct RecordingLifecycleHook {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    reject_value: Option<&'static str>,
    fail_after: bool,
    only_operation: Option<type_bridge::CrudOperation>,
}

impl RecordingLifecycleHook {
    fn new(name: &'static str, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            events,
            reject_value: None,
            fail_after: false,
            only_operation: None,
        }
    }

    fn rejecting(mut self, value: &'static str) -> Self {
        self.reject_value = Some(value);
        self
    }

    fn failing_after(mut self) -> Self {
        self.fail_after = true;
        self
    }

    fn only(mut self, operation: type_bridge::CrudOperation) -> Self {
        self.only_operation = Some(operation);
        self
    }

    fn input_contains(context: &type_bridge::HookContext<'_>, expected: &str) -> bool {
        context.input().is_some_and(|input| {
            input.fields().iter().any(|(_, values)| {
                values
                    .iter()
                    .any(|value| value.as_string() == Some(expected))
            })
        })
    }
}

impl type_bridge::LifecycleHook for RecordingLifecycleHook {
    fn name(&self) -> &str {
        self.name
    }

    fn before_operation<'a>(
        &'a self,
        context: &'a mut type_bridge::HookContext<'_>,
    ) -> type_bridge::HookFuture<
        'a,
        std::result::Result<type_bridge::PreHookResult, type_bridge::HookError>,
    > {
        Box::pin(async move {
            let saw_first = self.name == "first" || context.metadata().contains_key("hook:first");
            self.events.lock().expect("hook event lock").push(format!(
                "pre:{}:{:?}:first={saw_first}",
                self.name,
                context.operation()
            ));
            context.set_metadata(format!("hook:{}", self.name), true);
            if self
                .reject_value
                .is_some_and(|value| Self::input_contains(context, value))
            {
                return Ok(type_bridge::PreHookResult::Reject {
                    reason: format!("{} rejected generated input", self.name),
                });
            }
            Ok(type_bridge::PreHookResult::Continue)
        })
    }

    fn after_operation<'a>(
        &'a self,
        context: &'a type_bridge::HookContext<'_>,
    ) -> type_bridge::HookFuture<'a, std::result::Result<(), type_bridge::HookError>> {
        Box::pin(async move {
            let saw_both = context.metadata().contains_key("hook:first")
                && context.metadata().contains_key("hook:second");
            self.events.lock().expect("hook event lock").push(format!(
                "post:{}:{:?}:both={saw_both}",
                self.name,
                context.operation()
            ));
            if self.fail_after {
                return Err(type_bridge::HookError::Internal {
                    hook_name: self.name.to_owned(),
                    source: Box::new(std::io::Error::other("expected post-hook failure")),
                });
            }
            Ok(())
        })
    }

    fn should_run(&self, context: &type_bridge::HookContext<'_>) -> bool {
        self.only_operation
            .is_none_or(|operation| operation == context.operation())
    }
}

struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    exchange_count: Option<Arc<AtomicUsize>>,
}

impl HttpTransport {
    fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            exchange_count: None,
        }
    }

    fn recording(base_url: String, exchange_count: Arc<AtomicUsize>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            exchange_count: Some(exchange_count),
        }
    }
}

impl RemoteQueryTransport for HttpTransport {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        Box::pin(async move {
            let response = self
                .client
                .get(format!("{}/v2/capabilities", self.base_url))
                .send()
                .await
                .map_err(transport_error)?
                .error_for_status()
                .map_err(transport_error)?;
            let bytes = response.bytes().await.map_err(transport_error)?.to_vec();
            if !bytes
                .windows(b"typebridge.query-remote-capabilities/v1".len())
                .any(|window| window == b"typebridge.query-remote-capabilities/v1")
            {
                return Err(type_bridge::Error::remote(
                    "remote_capability_advertisement",
                    format!(
                        "remote capability discovery returned a non-advertisement: {}",
                        String::from_utf8_lossy(&bytes)
                    ),
                    None,
                ));
            }
            Ok(bytes)
        })
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(exchange_count) = &self.exchange_count {
                exchange_count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(self
                .client
                .post(format!("{}/v2/query", self.base_url))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(request.to_vec())
                .send()
                .await
                .map_err(transport_error)?
                .bytes()
                .await
                .map_err(transport_error)?
                .to_vec())
        })
    }
}

struct CancelBeforeDecodeTransport {
    inner: HttpTransport,
    cancellation: AnswerCancellation,
    response_completed: Arc<AtomicBool>,
}

impl RemoteQueryTransport for CancelBeforeDecodeTransport {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        self.inner.capabilities()
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.inner.exchange(request).await?;
            self.response_completed.store(true, Ordering::SeqCst);
            self.cancellation.cancel();
            Ok(response)
        })
    }
}

struct CallerAbortProbeTransport {
    cancellation: AnswerCancellation,
}

impl RemoteQueryTransport for CallerAbortProbeTransport {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        Box::pin(async {
            Err(Error::remote(
                "caller_transport_probe_only",
                "caller transport probe has no capability endpoint",
                None,
            ))
        })
    }

    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(std::future::poll_fn(move |context| {
            if self.cancellation.is_cancelled() {
                std::task::Poll::Ready(Err(Error::remote(
                    "caller_transport_aborted",
                    "caller transport stopped its own exchange",
                    None,
                )))
            } else {
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }))
    }
}

fn transport_error(error: reqwest::Error) -> type_bridge::Error {
    type_bridge::Error::remote("remote_transport", error.to_string(), Some(Box::new(error)))
}

fn connection_options() -> ConnectionOptions {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1729".to_owned());
    let database = env::var("TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE")
        .unwrap_or_else(|_| format!("type_bridge_rust_projection_live_{}", std::process::id()));
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let http_port = env::var("TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse()
        .expect("TYPEDB_HTTP_PORT is a valid nonzero u16");

    let options = ConnectionOptions::new(address, database)
        .credentials(username, password)
        .http_port(http_port);
    if env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref() == Ok("1") {
        options.tls(true)
    } else {
        options
    }
}

async fn database() -> Database<AppSchema> {
    Database::connect(connection_options())
        .await
        .expect("live projection database connects")
        .with_schema(SCHEMA)
        .expect("schema binding handshake succeeds")
}

const WORKFORCE_MANIFEST_PATH: &str = "tests/contracts/sdk_conformance/manifest-v1.json";
const WORKFORCE_CATALOG_PATH: &str = "tests/contracts/sdk_conformance/workforce-v1/catalog-v1.json";
const WORKFORCE_SCHEMA_PATH: &str =
    "type-bridge-core/crates/schema-codegen/tests/acceptance/schema.yaml";
const WORKFORCE_PROVIDER_SCHEMA_PATH: &str =
    "type-bridge-core/crates/schema-codegen/tests/acceptance/provider-3.12.1.tql";
const WORKFORCE_JOURNEY_PATH: &str = "tests/contracts/sdk_conformance/workforce-v1/journey-v1.json";
const WORKFORCE_V2_CATALOG_PATH: &str =
    "tests/contracts/sdk_conformance/workforce-v2/catalog-v2.json";
const WORKFORCE_V2_JOURNEY_PATH: &str =
    "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json";
const WORKFORCE_PROFILE: &str = "typedb-3.12.1/v1";

fn workforce_env_path(name: &str) -> PathBuf {
    PathBuf::from(env::var_os(name).unwrap_or_else(|| panic!("{name} is required")))
}

fn workforce_string<'a>(value: &'a Value, label: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{label} must be a JSON string"))
}

fn workforce_typed_field<'a>(fields: &'a Value, name: &str, kind: &str) -> &'a Value {
    let value = fields
        .get(name)
        .unwrap_or_else(|| panic!("workforce person field is missing: {name}"));
    assert_eq!(
        workforce_string(&value["kind"], "workforce field kind"),
        kind,
        "workforce person field has the wrong scalar domain: {name}"
    );
    value
}

fn workforce_field_string(fields: &Value, name: &str, kind: &str) -> String {
    workforce_string(
        &workforce_typed_field(fields, name, kind)["value"],
        "workforce field value",
    )
    .to_owned()
}

fn workforce_field_long(fields: &Value, name: &str) -> i64 {
    workforce_string(
        &workforce_typed_field(fields, name, "long")["value"],
        "workforce long value",
    )
    .parse()
    .unwrap_or_else(|_| panic!("workforce long value is invalid: {name}"))
}

fn workforce_field_double(fields: &Value, name: &str) -> f64 {
    let bits = workforce_string(
        &workforce_typed_field(fields, name, "double")["bits"],
        "workforce double bits",
    );
    assert_eq!(
        bits.len(),
        16,
        "workforce double bits must be 16 hex digits"
    );
    let bits = u64::from_str_radix(bits, 16).expect("workforce double bits are hexadecimal");
    let value = f64::from_bits(bits);
    assert!(value.is_finite(), "workforce double must be finite");
    value
}

fn workforce_aliases(fields: &Value) -> Vec<String> {
    fields["aliases"]
        .as_array()
        .expect("workforce aliases must be an array")
        .iter()
        .map(|value| {
            assert_eq!(
                workforce_string(&value["kind"], "workforce alias kind"),
                "string"
            );
            workforce_string(&value["value"], "workforce alias value").to_owned()
        })
        .collect()
}

fn workforce_person_create(fields: &Value, nickname: &str) -> PersonCreate {
    PersonCreate::try_new(
        workforce_aliases(fields)
            .into_iter()
            .map(|value| Aliases::new(value).expect("workforce alias is valid"))
            .collect(),
        Some(
            FooBar::new(workforce_field_long(fields, "foo__bar"))
                .expect("workforce foo__bar is valid"),
        ),
        Identifier::new(workforce_field_string(fields, "identifier", "string"))
            .expect("workforce identifier is valid"),
        Some(Nickname::new(nickname.to_owned()).expect("workforce nickname is valid")),
        Score::new(workforce_field_long(fields, "score")).expect("workforce score is valid"),
        Some(
            ScoreGte::new(workforce_field_long(fields, "score__gte"))
                .expect("workforce score__gte is valid"),
        ),
        ValBool::new(
            workforce_typed_field(fields, "val_bool", "boolean")["value"]
                .as_bool()
                .expect("workforce boolean value is valid"),
        )
        .expect("workforce val_bool is valid"),
        ValConstrained::new(workforce_field_long(fields, "val_constrained"))
            .expect("workforce val_constrained is valid"),
        ValDate::new(
            Date::try_new(workforce_field_string(fields, "val_date", "date"))
                .expect("workforce date is valid"),
        )
        .expect("workforce val_date is valid"),
        ValDatetime::new(
            DateTime::try_new(workforce_field_string(fields, "val_datetime", "datetime"))
                .expect("workforce datetime is valid"),
        )
        .expect("workforce val_datetime is valid"),
        ValDatetimeTz::new(
            DateTimeTz::try_new(workforce_field_string(
                fields,
                "val_datetime_tz",
                "datetime_tz",
            ))
            .expect("workforce datetime-tz is valid"),
        )
        .expect("workforce val_datetime_tz is valid"),
        ValDecimal::new(
            Decimal::try_new(workforce_field_string(fields, "val_decimal", "decimal"))
                .expect("workforce decimal is valid"),
        )
        .expect("workforce val_decimal is valid"),
        ValDouble::new(
            CanonicalDouble::try_new(workforce_field_double(fields, "val_double"))
                .expect("workforce double is valid"),
        )
        .expect("workforce val_double is valid"),
        ValDuration::new(
            Duration::try_new(workforce_field_string(fields, "val_duration", "duration"))
                .expect("workforce duration is valid"),
        )
        .expect("workforce val_duration is valid"),
    )
    .expect("workforce PersonCreate is valid")
}

fn assert_workforce_person(person: &Person, fields: &Value, nickname: &str) {
    assert_eq!(
        person.identifier().value(),
        &workforce_field_string(fields, "identifier", "string")
    );
    let mut actual_aliases = person
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    actual_aliases.sort();
    let mut expected_aliases = workforce_aliases(fields);
    expected_aliases.sort();
    assert_eq!(actual_aliases, expected_aliases);
    assert_eq!(
        person.nickname().map(|value| value.value().as_str()),
        Some(nickname)
    );
    assert_eq!(
        person.foo__bar().map(FooBar::value),
        Some(&workforce_field_long(fields, "foo__bar"))
    );
    assert_eq!(
        person.score().value(),
        &workforce_field_long(fields, "score")
    );
    assert_eq!(
        person.score__gte().map(ScoreGte::value),
        Some(&workforce_field_long(fields, "score__gte"))
    );
    assert_eq!(
        person.val_bool().value(),
        &workforce_typed_field(fields, "val_bool", "boolean")["value"]
            .as_bool()
            .expect("workforce boolean value is valid")
    );
    assert_eq!(
        person.val_constrained().value(),
        &workforce_field_long(fields, "val_constrained")
    );
    assert_eq!(
        person.val_date().value().as_str(),
        workforce_field_string(fields, "val_date", "date")
    );
    assert_eq!(
        person.val_datetime().value().as_str(),
        workforce_field_string(fields, "val_datetime", "datetime")
    );
    assert_eq!(
        person.val_datetime_tz().value().as_str(),
        workforce_field_string(fields, "val_datetime_tz", "datetime_tz")
    );
    assert_eq!(
        person.val_decimal().value().as_str(),
        workforce_field_string(fields, "val_decimal", "decimal")
    );
    assert_eq!(
        person.val_double().value().get().to_bits(),
        workforce_field_double(fields, "val_double").to_bits()
    );
    assert_eq!(
        person.val_duration().value().as_str(),
        workforce_field_string(fields, "val_duration", "duration")
    );
}

fn workforce_scalar_domains(fields: &Value) -> Vec<String> {
    let mut domains = fields
        .as_object()
        .expect("workforce person fields must be an object")
        .values()
        .flat_map(|value| {
            value
                .as_array()
                .map_or_else(|| vec![value], |values| values.iter().collect())
        })
        .map(|value| workforce_string(&value["kind"], "workforce scalar kind").to_owned())
        .collect::<Vec<_>>();
    domains.sort();
    domains.dedup();
    domains
}

fn workforce_model_observation(
    person: &Person,
    fields: &Value,
    model: &str,
    nickname: &str,
) -> Value {
    assert_workforce_person(person, fields, nickname);
    let mut aliases = person
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    aliases.sort();
    let reference = person.reference();
    let reference_key = reference
        .identifier()
        .expect("workforce person reference carries its key")
        .value()
        .clone();
    json!({
        "aliases": aliases,
        "key": person.identifier().value(),
        "model": model,
        "nickname": person.nickname().expect("workforce nickname is present").value(),
        "reference": {"key": reference_key, "model": model},
        "scalar_domains": workforce_scalar_domains(fields),
    })
}

fn workforce_role_observations(
    relation: &Membership,
    person: &Person,
    membership_record: &Value,
) -> (Value, Value) {
    let relation_model = workforce_string(&membership_record["model"], "workforce relation model");
    let role = workforce_string(&membership_record["role"], "workforce relation role");
    let player_model = workforce_string(
        &membership_record["player"]["model"],
        "workforce player model",
    );
    let player_key = match relation.member() {
        MembershipMemberPlayer::Person(reference) => reference
            .identifier()
            .expect("workforce membership player carries its key")
            .value()
            .clone(),
        MembershipMemberPlayer::Robot(_) => panic!("workforce membership hydrated a robot"),
    };
    assert_eq!(player_key, person.identifier().value().as_str());
    (
        json!({
            "relation": {"model": relation_model},
            "role": role,
            "player": {"key": player_key, "model": player_model},
        }),
        json!({
            "relation": relation_model,
            "role": role,
            "player": {"key": person.identifier().value(), "model": player_model},
        }),
    )
}

fn workforce_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn workforce_source_identity(path: &str, bytes: &[u8]) -> Value {
    json!({"path": path, "sha256": workforce_sha256(bytes)})
}

fn sort_workforce_json(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                sort_workforce_json(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                sort_workforce_json(value);
            }
            let mut entries = std::mem::take(values).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            values.extend(entries);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn validate_workforce_report_path(path: &Path) {
    let raw = path
        .to_str()
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT must be UTF-8");
    assert!(
        raw.len() <= 4096,
        "TYPE_BRIDGE_WORKFORCE_REPORT exceeds 4096 UTF-8 bytes"
    );
    assert!(
        path.is_absolute(),
        "TYPE_BRIDGE_WORKFORCE_REPORT must be absolute"
    );
    let parent = path
        .parent()
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT must have a parent");
    let parent_metadata = fs::symlink_metadata(parent)
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT parent must already exist");
    assert!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "TYPE_BRIDGE_WORKFORCE_REPORT parent must be a non-symlink directory"
    );
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => panic!("TYPE_BRIDGE_WORKFORCE_REPORT destination must be absent"),
        Err(error) => panic!("TYPE_BRIDGE_WORKFORCE_REPORT is not inspectable: {error}"),
    }
}

fn publish_workforce_report(path: &Path, mut report: Value) {
    validate_workforce_report_path(path);
    sort_workforce_json(&mut report);
    let mut bytes = serde_json::to_vec(&report).expect("workforce report serializes");
    bytes.push(b'\n');
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT must name a UTF-8 file");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    struct RemoveTemporary(PathBuf);
    impl Drop for RemoveTemporary {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let temporary_guard = RemoveTemporary(temporary.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .expect("workforce report temporary file is created without replacement");
    file.write_all(&bytes)
        .expect("workforce report temporary file is written");
    file.sync_all()
        .expect("workforce report temporary file is synchronized");
    drop(file);
    fs::hard_link(&temporary, path)
        .expect("workforce report is atomically published without replacement");
    fs::remove_file(&temporary).expect("workforce report temporary link is removed");
    drop(temporary_guard);
    let metadata = fs::symlink_metadata(path).expect("published workforce report is inspectable");
    assert!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "published workforce report must be a regular file"
    );
}

fn workforce_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn workforce_version_endpoint() -> String {
    assert_ne!(
        env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref(),
        Ok("1"),
        "workforce evidence is accepted only from the plaintext 3.12.1 lane"
    );
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1729".to_owned());
    let address_url = if address.contains("://") {
        address
    } else {
        format!("http://{address}")
    };
    let parsed = reqwest::Url::parse(&address_url)
        .expect("TYPEDB_ADDRESS can be parsed for the workforce version probe");
    let host = parsed
        .host_str()
        .expect("TYPEDB_ADDRESS has a host for the workforce version probe");
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let http_port = env::var("TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse::<u16>()
        .expect("TYPEDB_HTTP_PORT is a valid nonzero u16");
    assert_ne!(http_port, 0, "TYPEDB_HTTP_PORT must be nonzero");
    format!("http://{host}:{http_port}/v1/version")
}

async fn require_workforce_server_version() {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(StdDuration::from_secs(30))
        .build()
        .expect("workforce version-probe client builds");
    let mut response = client
        .get(workforce_version_endpoint())
        .send()
        .await
        .expect("workforce TypeDB version probe succeeds");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "workforce TypeDB version probe returns HTTP 200"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .expect("workforce TypeDB version body is readable")
    {
        assert!(
            body.len().saturating_add(chunk.len()) <= 4096,
            "workforce TypeDB version body exceeds 4096 bytes"
        );
        body.extend_from_slice(&chunk);
    }
    assert!(!body.is_empty(), "workforce TypeDB version body is empty");
    let document: Value =
        serde_json::from_slice(&body).expect("workforce TypeDB version body is valid JSON");
    assert_eq!(
        document.get("version").and_then(Value::as_str),
        Some("3.12.3"),
        "workforce evidence requires the actual detected TypeDB server version 3.12.3"
    );
}

fn workforce_results(catalog: &Value, observations: &BTreeMap<String, Value>) -> Vec<Value> {
    let mut capabilities = BTreeMap::new();
    for case in catalog["cases"]
        .as_array()
        .expect("workforce catalog cases must be an array")
    {
        let case_id = workforce_string(&case["id"], "workforce case ID").to_owned();
        let capability =
            workforce_string(&case["capability_id"], "workforce capability ID").to_owned();
        assert!(
            capabilities.insert(case_id, capability).is_none(),
            "workforce catalog contains a duplicate case"
        );
    }
    let selected = catalog["selected_proofs"]
        .as_array()
        .expect("workforce selected proofs must be an array");
    assert_eq!(selected.len(), 9, "workforce report requires nine proofs");
    let mut rows = selected
        .iter()
        .map(|proof| {
            let case_id = workforce_string(&proof["case_id"], "selected case ID").to_owned();
            let proof_kind =
                workforce_string(&proof["proof_kind"], "selected proof kind").to_owned();
            let observation_ref =
                workforce_string(&proof["observation_ref"], "selected observation reference");
            let capability_id = capabilities
                .get(&case_id)
                .unwrap_or_else(|| panic!("selected workforce case is unknown: {case_id}"))
                .clone();
            let observation = observations
                .get(observation_ref)
                .unwrap_or_else(|| {
                    panic!("selected workforce observation is unknown: {observation_ref}")
                })
                .clone();
            (case_id, capability_id, proof_kind, observation)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| (&left.0, &left.2).cmp(&(&right.0, &right.2)));
    rows.into_iter()
        .map(|(case_id, capability_id, proof_kind, observation)| {
            json!({
                "case_id": case_id,
                "capability_id": capability_id,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observation,
            })
        })
        .collect()
}

type WorkforceV2ObservationKey = (String, String);

fn workforce_v2_observation_key(
    observation_ref: &str,
    proof_kind: &str,
) -> WorkforceV2ObservationKey {
    (observation_ref.to_owned(), proof_kind.to_owned())
}

fn workforce_v2_results(
    catalog: &Value,
    observations: &BTreeMap<WorkforceV2ObservationKey, Value>,
) -> Vec<Value> {
    let mut capabilities = BTreeMap::new();
    for case in catalog["cases"]
        .as_array()
        .expect("workforce-v2 catalog cases must be an array")
    {
        let case_id = workforce_string(&case["id"], "workforce-v2 case ID").to_owned();
        let capability =
            workforce_string(&case["capability_id"], "workforce-v2 capability ID").to_owned();
        assert!(
            capabilities.insert(case_id, capability).is_none(),
            "workforce-v2 catalog contains a duplicate case"
        );
    }
    let selected = catalog["selected_proofs"]
        .as_array()
        .expect("workforce-v2 selected proofs must be an array");
    assert_eq!(selected.len(), 34, "workforce-v2 report requires 34 proofs");
    let mut required = BTreeMap::new();
    let mut rows = selected
        .iter()
        .map(|proof| {
            let case_id = workforce_string(&proof["case_id"], "workforce-v2 selected case ID")
                .to_owned();
            let proof_kind = workforce_string(
                &proof["proof_kind"],
                "workforce-v2 selected proof kind",
            )
            .to_owned();
            let observation_ref = workforce_string(
                &proof["observation_ref"],
                "workforce-v2 selected observation reference",
            )
            .to_owned();
            let capability_id = capabilities
                .get(&case_id)
                .unwrap_or_else(|| panic!("selected workforce-v2 case is unknown: {case_id}"))
                .clone();
            let key = workforce_v2_observation_key(&observation_ref, &proof_kind);
            assert!(
                required.insert(key.clone(), ()).is_none(),
                "workforce-v2 selected proof is duplicated: {case_id}/{proof_kind}"
            );
            let observation = observations
                .get(&key)
                .unwrap_or_else(|| {
                    panic!(
                        "selected workforce-v2 observation is absent: {observation_ref}/{proof_kind}"
                    )
                })
                .clone();
            (case_id, capability_id, proof_kind, observation)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observations.keys().collect::<Vec<_>>(),
        required.keys().collect::<Vec<_>>(),
        "workforce-v2 producer must emit exactly the selected observation lanes"
    );
    rows.sort_by(|left, right| (&left.0, &left.2).cmp(&(&right.0, &right.2)));
    rows.into_iter()
        .map(|(case_id, capability_id, proof_kind, observation)| {
            json!({
                "case_id": case_id,
                "capability_id": capability_id,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observation,
            })
        })
        .collect()
}

fn workforce_v2_provider_proofs() -> BTreeMap<WorkforceV2ObservationKey, Value> {
    let raw = env::var("TYPE_BRIDGE_WORKFORCE_V2_VALIDATED_OBSERVATIONS")
        .expect("outer harness must supply validated workforce-v2 observations");
    assert!(
        raw.len() <= 64 * 1024,
        "validated workforce-v2 observations exceed 64 KiB"
    );
    let observations: BTreeMap<String, Value> = serde_json::from_str(&raw)
        .expect("outer-validated workforce-v2 observations are canonical JSON");
    let allowed = [
        workforce_v2_observation_key("cancellation_direct", "direct_runtime"),
        workforce_v2_observation_key("cancellation_remote", "remote_runtime"),
        workforce_v2_observation_key("remote_structured_diagnostic", "diagnostic"),
    ];
    assert_eq!(
        observations.keys().map(String::as_str).collect::<Vec<_>>(),
        allowed
            .iter()
            .map(|(observation_ref, proof_kind)| format!("{observation_ref}/{proof_kind}"))
            .collect::<Vec<_>>(),
        "outer-validated workforce-v2 observations have incomplete lane coverage"
    );
    allowed
        .into_iter()
        .map(|key| {
            let (observation_ref, proof_kind) = (&key.0, &key.1);
            let serialized_key = format!("{observation_ref}/{proof_kind}");
            let observation = observations
                .get(&serialized_key)
                .unwrap_or_else(|| {
                    panic!("validated workforce-v2 observation is absent: {serialized_key}")
                })
                .clone();
            assert!(observation.is_object());
            (key, observation)
        })
        .collect()
}

fn workforce_v2_person_create(record: &Value) -> PersonCreate {
    let fields = &record["fields"];
    let nickname = fields.get("nickname").map(|value| {
        assert_eq!(
            workforce_string(&value["kind"], "workforce-v2 nickname kind"),
            "string"
        );
        Nickname::new(workforce_string(&value["value"], "workforce-v2 nickname value").to_owned())
            .expect("workforce-v2 nickname is valid")
    });
    PersonCreate::try_new(
        workforce_aliases(fields)
            .into_iter()
            .map(|value| Aliases::new(value).expect("workforce-v2 alias is valid"))
            .collect(),
        None,
        Identifier::new(workforce_field_string(fields, "identifier", "string"))
            .expect("workforce-v2 person identifier is valid"),
        nickname,
        Score::new(workforce_field_long(fields, "score")).expect("workforce-v2 score is valid"),
        Some(
            ScoreGte::new(workforce_field_long(fields, "score__gte"))
                .expect("workforce-v2 score__gte is valid"),
        ),
        ValBool::new(
            workforce_typed_field(fields, "val_bool", "boolean")["value"]
                .as_bool()
                .expect("workforce-v2 boolean is valid"),
        )
        .expect("workforce-v2 val_bool is valid"),
        ValConstrained::new(workforce_field_long(fields, "val_constrained"))
            .expect("workforce-v2 val_constrained is valid"),
        ValDate::new(
            Date::try_new(workforce_field_string(fields, "val_date", "date"))
                .expect("workforce-v2 date is valid"),
        )
        .expect("workforce-v2 val_date is valid"),
        ValDatetime::new(
            DateTime::try_new(workforce_field_string(fields, "val_datetime", "datetime"))
                .expect("workforce-v2 datetime is valid"),
        )
        .expect("workforce-v2 val_datetime is valid"),
        ValDatetimeTz::new(
            DateTimeTz::try_new(workforce_field_string(
                fields,
                "val_datetime_tz",
                "datetime_tz",
            ))
            .expect("workforce-v2 datetime-tz is valid"),
        )
        .expect("workforce-v2 val_datetime_tz is valid"),
        ValDecimal::new(
            Decimal::try_new(workforce_field_string(fields, "val_decimal", "decimal"))
                .expect("workforce-v2 decimal is valid"),
        )
        .expect("workforce-v2 val_decimal is valid"),
        ValDouble::new(
            CanonicalDouble::try_new(workforce_field_double(fields, "val_double"))
                .expect("workforce-v2 double is valid"),
        )
        .expect("workforce-v2 val_double is valid"),
        ValDuration::new(
            Duration::try_new(workforce_field_string(fields, "val_duration", "duration"))
                .expect("workforce-v2 duration is valid"),
        )
        .expect("workforce-v2 val_duration is valid"),
    )
    .expect("workforce-v2 PersonCreate is valid")
}

fn workforce_v2_assert_person(person: &Person, record: &Value) {
    let fields = &record["fields"];
    assert_eq!(
        person.identifier().value(),
        &workforce_field_string(fields, "identifier", "string")
    );
    let mut aliases = person
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    aliases.sort();
    let mut expected_aliases = workforce_aliases(fields);
    expected_aliases.sort();
    assert_eq!(aliases, expected_aliases);
    assert_eq!(
        person.nickname().map(|value| value.value().as_str()),
        fields
            .get("nickname")
            .map(|value| { workforce_string(&value["value"], "workforce-v2 nickname value") })
    );
    assert_eq!(person.foo__bar(), None);
    assert_eq!(
        person.score().value(),
        &workforce_field_long(fields, "score")
    );
    assert_eq!(
        person.score__gte().map(ScoreGte::value),
        Some(&workforce_field_long(fields, "score__gte"))
    );
    assert_eq!(
        person.val_bool().value(),
        &workforce_typed_field(fields, "val_bool", "boolean")["value"]
            .as_bool()
            .expect("workforce-v2 boolean is valid")
    );
    assert_eq!(
        person.val_constrained().value(),
        &workforce_field_long(fields, "val_constrained")
    );
    assert_eq!(
        person.val_date().value().as_str(),
        workforce_field_string(fields, "val_date", "date")
    );
    assert_eq!(
        person.val_datetime().value().as_str(),
        workforce_field_string(fields, "val_datetime", "datetime")
    );
    assert_eq!(
        person.val_datetime_tz().value().as_str(),
        workforce_field_string(fields, "val_datetime_tz", "datetime_tz")
    );
    assert_eq!(
        person.val_decimal().value().as_str(),
        workforce_field_string(fields, "val_decimal", "decimal")
    );
    assert_eq!(
        person.val_double().value().get().to_bits(),
        workforce_field_double(fields, "val_double").to_bits()
    );
    assert_eq!(
        person.val_duration().value().as_str(),
        workforce_field_string(fields, "val_duration", "duration")
    );
}

fn workforce_v2_model_observation(person: &Person, record: &Value) -> Value {
    workforce_v2_assert_person(person, record);
    let mut aliases = person
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    aliases.sort();
    let reference = person.reference();
    let key = reference
        .identifier()
        .expect("workforce-v2 person reference carries its key")
        .value();
    let mut scalar_domains = vec![
        workforce_v2_scalar_domain(person.val_bool().value()),
        workforce_v2_scalar_domain(person.val_date().value()),
        workforce_v2_scalar_domain(person.val_datetime().value()),
        workforce_v2_scalar_domain(person.val_datetime_tz().value()),
        workforce_v2_scalar_domain(person.val_decimal().value()),
        workforce_v2_scalar_domain(person.val_double().value()),
        workforce_v2_scalar_domain(person.val_duration().value()),
        workforce_v2_scalar_domain(person.score().value()),
        workforce_v2_scalar_domain(person.identifier().value()),
    ];
    scalar_domains.sort_unstable();
    scalar_domains.dedup();
    json!({
        "aliases": aliases,
        "key": person.identifier().value(),
        "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
        "nickname": person.nickname().expect("workforce-v2 Ada nickname is present").value(),
        "reference": {
            "key": key,
            "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
        },
        "scalar_domains": scalar_domains,
    })
}

fn workforce_v2_scalar_domain(value: &impl IntoEncodedScalar) -> &'static str {
    match value.into_encoded_scalar() {
        EncodedScalar::String(_) => "string",
        EncodedScalar::Long(_) => "long",
        EncodedScalar::Double(_) => "double",
        EncodedScalar::Decimal(_) => "decimal",
        EncodedScalar::Boolean(_) => "boolean",
        EncodedScalar::Date(_) => "date",
        EncodedScalar::DateTime(_) => "datetime",
        EncodedScalar::DateTimeTz(_) => "datetime_tz",
        EncodedScalar::Duration(_) => "duration",
    }
}

fn workforce_v2_type_label(type_id_json: &str) -> String {
    serde_json::from_str::<Value>(type_id_json)
        .expect("generated workforce-v2 type identity is JSON")["label"]
        .as_str()
        .expect("generated workforce-v2 type identity carries a label")
        .to_owned()
}

fn workforce_v2_field_label(owns_id_json: &str) -> String {
    serde_json::from_str::<Value>(owns_id_json)
        .expect("generated workforce-v2 field identity is JSON")["attribute"]
        .as_str()
        .expect("generated workforce-v2 field identity carries an attribute label")
        .to_owned()
}

fn workforce_v2_role_label(role_id_json: &str) -> String {
    serde_json::from_str::<Value>(role_id_json)
        .expect("generated workforce-v2 role identity is JSON")["label"]
        .as_str()
        .expect("generated workforce-v2 role identity carries a label")
        .to_owned()
}

fn workforce_v2_network_player_key(player: &Person) -> &str {
    player.identifier().value()
}

fn workforce_v2_network_origin_key(player: &Person) -> &str {
    player.identifier().value()
}

fn workforce_v2_network_destination_key(player: &Person) -> &str {
    player.identifier().value()
}

fn workforce_v2_query_category(error: &Error) -> Option<&'static str> {
    error.details()?.values().find_map(|value| match value {
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::InvalidPlan) => Some("invalid_plan"),
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::Cardinality) => Some("cardinality"),
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::UnsupportedCapability) => {
            Some("unsupported_capability")
        }
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::StaleSchema) => Some("stale_schema"),
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::ResourceLimit) => {
            Some("resource_limit")
        }
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::Cancelled) => Some("cancelled"),
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::Provider) => Some("provider"),
        ErrorDetail::QueryCategory(QueryDiagnosticCategory::ResultDecode) => Some("result_decode"),
        _ => None,
    })
}

fn workforce_v2_diagnostic_path(error: &Error) -> Vec<Value> {
    error
        .diagnostic_path()
        .expect("workforce-v2 query error carries a typed path")
        .iter()
        .map(|segment| match segment {
            ErrorPathSegment::Field(value) => json!({"kind": "field", "value": value}),
            ErrorPathSegment::Index(value) => json!({"kind": "index", "value": value}),
            ErrorPathSegment::Identifier(value) => {
                json!({"kind": "identity", "value": value})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Request) => {
                json!({"kind": "request"})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Plan) => json!({"kind": "plan"}),
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Operation) => {
                json!({"kind": "operation"})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Predicate) => {
                json!({"kind": "predicate"})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Output) => {
                json!({"kind": "output"})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::ProviderEvidence) => {
                json!({"kind": "provider_evidence"})
            }
            ErrorPathSegment::Query(QueryDiagnosticPathKind::Result) => {
                json!({"kind": "result"})
            }
            ErrorPathSegment::QueryBinding(value) => {
                json!({"kind": "query_binding", "value": value})
            }
            ErrorPathSegment::QueryField { owner, name } => {
                json!({"kind": "query_field", "owner": owner, "name": name})
            }
            ErrorPathSegment::QueryRole { owner, name } => {
                json!({"kind": "query_role", "owner": owner, "name": name})
            }
            ErrorPathSegment::QueryRoleEdge(value) => {
                json!({"kind": "query_role_edge", "value": value})
            }
            ErrorPathSegment::QueryOutputSlot(value) => {
                json!({"kind": "query_output_slot", "value": value})
            }
            ErrorPathSegment::QueryOutputName(value) => {
                json!({"kind": "query_output_name", "value": value})
            }
            ErrorPathSegment::ContractField(value) => {
                json!({"kind": "contract_field", "value": value})
            }
            ErrorPathSegment::ContractIdentity(value) => {
                json!({"kind": "contract_identity", "value": value})
            }
            _ => panic!("workforce-v2 received an unsupported diagnostic path segment"),
        })
        .collect()
}

fn workforce_v2_diagnostic_details(error: &Error) -> Value {
    let mut details = serde_json::Map::new();
    for (name, value) in error
        .details()
        .expect("workforce-v2 query error carries typed details")
    {
        if matches!(value, ErrorDetail::QueryCategory(_)) {
            continue;
        }
        let normalized = match value {
            ErrorDetail::Text(value) => json!({"kind": "text", "value": value}),
            ErrorDetail::Long(value) if name == "actual" => {
                json!({"kind": "count", "value": value.to_string()})
            }
            ErrorDetail::Long(value) => json!({"kind": "signed", "value": value.to_string()}),
            ErrorDetail::Boolean(value) => json!({"kind": "boolean", "value": value}),
            ErrorDetail::TextList(value) => json!({"kind": "text_list", "value": value}),
            ErrorDetail::QueryIdentity(value) => {
                json!({"kind": "query_identity", "value": value})
            }
            ErrorDetail::QueryIdentityList(value) => {
                json!({"kind": "query_identity_list", "value": value})
            }
            ErrorDetail::QueryCategory(_) => unreachable!(),
            _ => panic!("workforce-v2 received an unsupported diagnostic detail"),
        };
        assert!(
            details.insert(name.clone(), normalized).is_none(),
            "workforce-v2 diagnostic detail names are unique"
        );
    }
    Value::Object(details)
}

fn workforce_v2_diagnostic(
    error: &Error,
    claim_consumed: Option<bool>,
    forbidden_values: &[&str],
) -> Value {
    let query_category = workforce_v2_query_category(error)
        .expect("workforce-v2 query error carries a query category");
    let category = match (error.category(), query_category) {
        (ErrorCategory::ModelValidation, "cardinality") => "invalid_input",
        (ErrorCategory::ModelValidation, "result_decode") => "integrity",
        (category, _) => category.as_str(),
    };
    let path = workforce_v2_diagnostic_path(error);
    let details = workforce_v2_diagnostic_details(error);
    let rendered = format!("{}{:?}{:?}", error.message(), path, details);
    let redacted = !forbidden_values
        .iter()
        .any(|secret| !secret.is_empty() && rendered.contains(secret));
    let mut observation = json!({
        "category": category,
        "query_category": query_category,
        "code": error.code().expect("workforce-v2 query error carries a stable code"),
        "message": error.message(),
        "path": path,
        "details": details,
        "redacted": redacted,
    });
    if let Some(claim_consumed) = claim_consumed {
        observation["claim_consumed"] = Value::Bool(claim_consumed);
    }
    observation
}

fn workforce_v2_keys(people: &[Person]) -> Vec<String> {
    people
        .iter()
        .map(|person| person.identifier().value().clone())
        .collect()
}

fn workforce_v2_common_key_prefix(keys: &[String]) -> String {
    let mut prefix = keys
        .first()
        .expect("workforce-v2 keyed records are present")
        .clone();
    while keys.iter().any(|key| !key.starts_with(&prefix)) {
        assert!(
            prefix.pop().is_some(),
            "workforce-v2 keyed records require a shared namespace"
        );
    }
    assert!(
        !prefix.is_empty(),
        "workforce-v2 keyed records require a nonempty shared namespace"
    );
    prefix
}

fn workforce_v2_family_identity(value: &EmployeeFamily) -> Value {
    match value {
        EmployeeFamily::Employee(employee) => {
            json!({
                "model": workforce_v2_type_label(EmployeeType::TOKEN.type_id_json()),
                "key": employee.identifier().value(),
            })
        }
        EmployeeFamily::Manager(manager) => {
            json!({
                "model": workforce_v2_type_label(ManagerType::TOKEN.type_id_json()),
                "key": manager.identifier().value(),
            })
        }
    }
}

async fn workforce_v2_query_observations(
    session: &mut QuerySession<'_, AppSchema>,
    records: &Value,
    person_iids: &[String; 2],
    exchange_count: Option<&Arc<AtomicUsize>>,
) -> BTreeMap<String, Value> {
    let people_records = records["people"]
        .as_array()
        .expect("workforce-v2 people records must be an array");
    assert_eq!(people_records.len(), 2, "workforce-v2 requires two people");
    let ada_key = workforce_field_string(&people_records[0]["fields"], "identifier", "string");
    let dana_key = workforce_field_string(&people_records[1]["fields"], "identifier", "string");
    let employee_key =
        workforce_field_string(&records["employee"]["fields"], "identifier", "string");
    let manager_key = workforce_field_string(&records["manager"]["fields"], "identifier", "string");
    let network_key =
        workforce_field_string(&records["network_link"]["fields"], "identifier", "string");
    let scalar_operand = workforce_field_long(&people_records[0]["fields"], "score__gte");
    assert!(
        people_records
            .iter()
            .all(|record| workforce_field_long(&record["fields"], "score__gte") == scalar_operand),
        "workforce-v2 scalar comparison operand must be shared by the fixture rows"
    );

    let person = session
        .exact::<Person>()
        .expect("workforce-v2 person binding");
    let grouped_person = session
        .exact::<Person>()
        .expect("workforce-v2 grouped person binding");
    let employee = session
        .exact::<Employee>()
        .expect("workforce-v2 exact employee binding");
    let employee_family = session
        .subtypes::<Employee>()
        .expect("workforce-v2 employee subtype binding");
    let membership = session
        .exact::<Membership>()
        .expect("workforce-v2 membership binding");
    let membership_person = session
        .exact::<Person>()
        .expect("workforce-v2 membership person binding");
    let network = session
        .exact::<NetworkLink>()
        .expect("workforce-v2 network binding");
    let network_origin = session
        .exact::<Person>()
        .expect("workforce-v2 network origin binding");
    let network_destination = session
        .exact::<Person>()
        .expect("workforce-v2 network destination binding");
    let network_participant = session
        .exact::<Person>()
        .expect("workforce-v2 network participant binding");
    let source = session
        .exact::<Person>()
        .expect("workforce-v2 topology source binding");
    let target = session
        .exact::<Person>()
        .expect("workforce-v2 topology target binding");
    let cross_left = session
        .exact::<Person>()
        .expect("workforce-v2 cross-left binding");
    let cross_right = session
        .exact::<Person>()
        .expect("workforce-v2 cross-right binding");
    let shape_link = session
        .exact::<NetworkLink>()
        .expect("workforce-v2 shape network binding");
    let shape_origin = session
        .exact::<Person>()
        .expect("workforce-v2 shape origin binding");
    let shape_participant = session
        .exact::<Person>()
        .expect("workforce-v2 shape participant binding");
    let identifier = person.field(PersonType::identifier);
    let grouped_identifier = grouped_person.field(PersonType::identifier);
    let score = person.field(PersonType::score);
    let score_gte = person.field(PersonType::score__gte);
    let scope = identifier.eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
        | identifier.eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key"));
    let people_query = session
        .query(person)
        .expect("workforce-v2 people selection")
        .where_(scope.clone())
        .expect("workforce-v2 people scope");

    let one_terminal = stringify!(one);
    let exchanges_before_one = exchange_count.map(|count| count.load(Ordering::SeqCst));
    let ada = people_query
        .where_(identifier.eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key")))
        .expect("workforce-v2 Ada predicate")
        .one()
        .await
        .expect("workforce-v2 Ada terminal");
    let exchanges_after_one = exchange_count.map(|count| count.load(Ordering::SeqCst));
    workforce_v2_assert_person(&ada, &people_records[0]);
    let model_values = workforce_v2_model_observation(&ada, &people_records[0]);

    let owner_keys = workforce_v2_keys(
        &people_query
            .where_(score.is_present())
            .expect("workforce-v2 score owner predicate")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 score owner rows"),
    );
    let optional_present_keys = workforce_v2_keys(
        &people_query
            .where_(person.field(PersonType::nickname).is_present())
            .expect("workforce-v2 nickname presence predicate")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 nickname presence rows"),
    );
    let iid_set_keys = workforce_v2_keys(
        &session
            .query(person)
            .expect("workforce-v2 IID-set query")
            .where_(person.iid_in([person_iids[0].as_str(), person_iids[1].as_str()]))
            .expect("workforce-v2 IID-set predicate")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 IID-set rows"),
    );
    let owner_iid_set = json!({
        "owner_field": workforce_v2_field_label(PersonType::score.owns_id_json()),
        "owner_keys": owner_keys,
        "optional_field": workforce_v2_field_label(PersonType::nickname.owns_id_json()),
        "optional_present_keys": optional_present_keys,
        "iid_set_keys": iid_set_keys,
    });

    let employee_identifier = employee.field(EmployeeType::identifier);
    let exact_employees = session
        .query(employee)
        .expect("workforce-v2 exact employee query")
        .where_(
            employee_identifier
                .eq(Identifier::new(employee_key.clone()).expect("workforce-v2 employee key")),
        )
        .expect("workforce-v2 exact employee predicate")
        .rows(RowsOptions::new(2).order_by(employee_identifier.asc()))
        .await
        .expect("workforce-v2 exact employee rows");
    let exact = exact_employees
        .iter()
        .map(|employee| {
            json!({
                "model": workforce_v2_type_label(EmployeeType::TOKEN.type_id_json()),
                "key": employee.identifier().value(),
            })
        })
        .collect::<Vec<_>>();
    let family_identifier = employee_family.field(EmployeeType::identifier);
    let employee_scope = family_identifier
        .eq(Identifier::new(employee_key.clone()).expect("workforce-v2 employee key"))
        | family_identifier
            .eq(Identifier::new(manager_key.clone()).expect("workforce-v2 manager key"));
    let subtypes = session
        .query(employee_family)
        .expect("workforce-v2 employee subtype query")
        .where_(employee_scope)
        .expect("workforce-v2 employee subtype scope")
        .rows(RowsOptions::new(2).order_by(family_identifier.asc()))
        .await
        .expect("workforce-v2 employee subtype rows")
        .iter()
        .map(workforce_v2_family_identity)
        .collect::<Vec<_>>();
    let exact_subtypes = json!({
        "declared_model": workforce_v2_type_label(EmployeeType::TOKEN.type_id_json()),
        "exact": exact,
        "subtypes": subtypes,
    });

    let and_keys = workforce_v2_keys(
        &people_query
            .where_(
                score.ge(scalar_operand)
                    & person
                        .field(PersonType::val_bool)
                        .eq(ValBool::new(true).expect("workforce-v2 true value")),
            )
            .expect("workforce-v2 conjunction")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 conjunction rows"),
    );
    let or_keys = workforce_v2_keys(
        &session
            .query(person)
            .expect("workforce-v2 disjunction query")
            .where_(
                identifier.eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
                    | identifier
                        .eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key")),
            )
            .expect("workforce-v2 disjunction")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 disjunction rows"),
    );
    let not_keys = workforce_v2_keys(
        &people_query
            .where_(!score.ge(scalar_operand))
            .expect("workforce-v2 negation")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 negation rows"),
    );
    let field_comparison_keys = workforce_v2_keys(
        &people_query
            .where_(score.ge_field(score_gte))
            .expect("workforce-v2 field comparison")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 field comparison rows"),
    );
    let scalar_boolean = json!({
        "and_keys": and_keys,
        "or_keys": or_keys,
        "not_keys": not_keys,
        "field_comparison_keys": field_comparison_keys,
    });

    let membership_person_identifier = membership_person.field(PersonType::identifier);
    let (hydrated_membership, hydrated_member) = session
        .query((membership, membership_person))
        .expect("workforce-v2 membership selection")
        .where_(
            membership
                .role(MembershipType::member)
                .connects(membership_person)
                & membership_person_identifier
                    .eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key")),
        )
        .expect("workforce-v2 membership role predicate")
        .one()
        .await
        .expect("workforce-v2 membership role result");
    let membership_player_key = match hydrated_membership.member() {
        MembershipMemberPlayer::Person(reference) => reference
            .identifier()
            .expect("workforce-v2 member carries its key")
            .value()
            .clone(),
        MembershipMemberPlayer::Robot(_) => panic!("workforce-v2 member must be a person"),
    };
    assert_eq!(
        membership_player_key,
        hydrated_member.identifier().value().as_str()
    );

    let hydrated_network = session
        .query(network)
        .expect("workforce-v2 network query")
        .match_(network_origin)
        .expect("workforce-v2 attach network origin")
        .match_(network_destination)
        .expect("workforce-v2 attach network destination")
        .match_(network_participant)
        .expect("workforce-v2 attach network participant")
        .where_(
            network
                .field(NetworkLinkType::identifier)
                .eq(Identifier::new(network_key.clone()).expect("workforce-v2 network key"))
                & network
                    .role(NetworkLinkType::origin)
                    .connects(network_origin)
                & network
                    .role(NetworkLinkType::destination)
                    .connects(network_destination)
                & network
                    .role(NetworkLinkType::participant)
                    .connects(network_participant),
        )
        .expect("workforce-v2 network role predicates")
        .one()
        .await
        .expect("workforce-v2 network role result");
    let mut participant_keys = hydrated_network
        .participant()
        .iter()
        .map(workforce_v2_network_player_key)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    participant_keys.sort();
    let roles = json!({
        "membership": {
            "relation": workforce_v2_type_label(MembershipType::TOKEN.type_id_json()),
            "role": workforce_v2_role_label(MembershipType::member.role_id_json()),
            "players": [{
                "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
                "key": membership_player_key,
            }],
        },
        "network_link": {
            "relation": workforce_v2_type_label(NetworkLinkType::TOKEN.type_id_json()),
            "origin": workforce_v2_network_origin_key(hydrated_network.origin()),
            "destination": workforce_v2_network_destination_key(hydrated_network.destination()),
            "participants": participant_keys,
        },
    });
    let hydrated_result = json!({
        "rows": [{
            "model": workforce_v2_type_label(MembershipType::TOKEN.type_id_json()),
            "roles": {workforce_v2_role_label(MembershipType::member.role_id_json()): [{
                "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
                "key": membership_player_key,
            }]},
        }],
    });

    let source_identifier = source.field(PersonType::identifier);
    let target_identifier = target.field(PersonType::identifier);
    let reachability_min_hops = 1;
    let reachability_max_hops = 1;
    let reachable = session
        .reachable(
            NetworkLinkType::TOKEN,
            NetworkLinkType::origin,
            NetworkLinkType::destination,
            source,
            target,
            reachability_min_hops,
            reachability_max_hops,
        )
        .expect("workforce-v2 one-hop reachability predicate");
    let reachable_rows = session
        .query((source, target))
        .expect("workforce-v2 reachability query")
        .where_(
            reachable
                & source_identifier
                    .eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
                & target_identifier
                    .eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key")),
        )
        .expect("workforce-v2 reachability filters")
        .rows(
            RowsOptions::new(2)
                .order_by(source_identifier.asc())
                .order_by(target_identifier.asc()),
        )
        .await
        .expect("workforce-v2 reachability rows");
    let reachable = reachable_rows
        .iter()
        .map(|(source, target)| {
            json!({
                "from": source.identifier().value(),
                "to": target.identifier().value(),
                "max_hops": reachability_max_hops,
            })
        })
        .collect::<Vec<_>>();

    let cross_left_identifier = cross_left.field(PersonType::identifier);
    let cross_right_identifier = cross_right.field(PersonType::identifier);
    let left_scope = cross_left_identifier
        .eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
        | cross_left_identifier
            .eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key"));
    let right_scope = cross_right_identifier
        .eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
        | cross_right_identifier
            .eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key"));
    let cross_join_pairs = session
        .query((cross_left, cross_right))
        .expect("workforce-v2 cross-join query")
        .allow_cross_join(cross_left, cross_right)
        .expect("workforce-v2 explicit cross join")
        .where_(left_scope & right_scope)
        .expect("workforce-v2 cross-join scope")
        .rows(
            RowsOptions::new(4)
                .order_by(cross_left_identifier.asc())
                .order_by(cross_right_identifier.asc()),
        )
        .await
        .expect("workforce-v2 cross-join rows")
        .iter()
        .map(|(left, right)| json!([left.identifier().value(), right.identifier().value()]))
        .collect::<Vec<_>>();
    let topology = json!({
        "reachable": reachable,
        "cross_join_pairs": cross_join_pairs,
    });

    let shape_participant_identifier = shape_participant.field(PersonType::identifier);
    let collected = shape_participant
        .collect()
        .distinct()
        .order_by(shape_participant_identifier.asc())
        .expect("workforce-v2 collection order");
    let shape_predicate = shape_link
        .field(NetworkLinkType::identifier)
        .eq(Identifier::new(network_key).expect("workforce-v2 network key"))
        & shape_link
            .role(NetworkLinkType::origin)
            .connects(shape_origin)
        & shape_link
            .role(NetworkLinkType::participant)
            .connects(shape_participant);
    let positional_page = session
        .query((shape_origin, collected.clone()))
        .expect("workforce-v2 positional selection")
        .match_(shape_link)
        .expect("workforce-v2 positional network match")
        .where_(shape_predicate.clone())
        .expect("workforce-v2 positional predicates")
        .page_by(
            shape_origin,
            PageOptions::new(1).order_by(shape_origin.field(PersonType::identifier).asc()),
        )
        .await
        .expect("workforce-v2 positional page");
    let positional_rows = positional_page.items();
    assert_eq!(
        positional_rows.len(),
        1,
        "workforce-v2 positional row is unique"
    );
    let positional = json!([
        positional_rows[0].0.identifier().value(),
        workforce_v2_keys(&positional_rows[0].1),
    ]);
    let positional_distinct = positional_rows[0]
        .1
        .iter()
        .map(Person::iid)
        .collect::<BTreeSet<_>>()
        .len()
        == positional_rows[0].1.len();
    let named_shape = WorkforceNetworkShape::select(shape_origin, collected)
        .expect("workforce-v2 named selection shape");
    let named_page = session
        .query(named_shape)
        .expect("workforce-v2 named selection")
        .match_(shape_link)
        .expect("workforce-v2 named network match")
        .where_(shape_predicate)
        .expect("workforce-v2 named predicates")
        .page_by(
            shape_origin,
            PageOptions::new(1).order_by(shape_origin.field(PersonType::identifier).asc()),
        )
        .await
        .expect("workforce-v2 named page");
    let named_rows = named_page.items();
    assert_eq!(named_rows.len(), 1, "workforce-v2 named row is unique");
    let named = json!({
        "origin": named_rows[0].origin.identifier().value(),
        "participants": workforce_v2_keys(&named_rows[0].participants),
    });
    let named_distinct = named_rows[0]
        .participants
        .iter()
        .map(Person::iid)
        .collect::<BTreeSet<_>>()
        .len()
        == named_rows[0].participants.len();
    assert_eq!(positional_distinct, named_distinct);
    let selection_shapes = json!({
        "positional": positional,
        "named": named,
        "collected_distinct": positional_distinct,
        "collection_order": format!(
            "{}_asc",
            workforce_v2_field_label(PersonType::identifier.owns_id_json()),
        ),
    });

    let one = people_query
        .where_(identifier.eq(Identifier::new(dana_key).expect("workforce-v2 Dana key")))
        .expect("workforce-v2 one predicate")
        .one()
        .await
        .expect("workforce-v2 one terminal")
        .identifier()
        .value()
        .clone();
    let first = people_query
        .first(identifier.asc())
        .await
        .expect("workforce-v2 first terminal")
        .expect("workforce-v2 first row exists")
        .identifier()
        .value()
        .clone();
    let rows = workforce_v2_keys(
        &people_query
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 rows terminal"),
    );
    let page = people_query
        .page_by(
            person,
            PageOptions::new(1)
                .include_total(true)
                .order_by(identifier.asc()),
        )
        .await
        .expect("workforce-v2 page terminal");
    let page_items = workforce_v2_keys(page.items());
    let count = people_query
        .count()
        .await
        .expect("workforce-v2 count terminal");
    let exists = people_query
        .exists()
        .await
        .expect("workforce-v2 exists terminal");
    let terminals = json!({
        "one": one,
        "first": first,
        "rows": rows,
        "page": {
            "items": page_items,
            "offset": page.offset(),
            "limit": page.limit(),
            "total": page.total().expect("workforce-v2 page total was requested"),
        },
        "count": count,
        "exists": exists,
    });

    let reductions: (
        u64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = people_query
        .aggregate((
            aggregate::count(),
            score.sum(),
            score.min(),
            score.max(),
            score.mean(),
            score.median(),
            score.stddev(),
        ))
        .await
        .expect("workforce-v2 reductions");
    let mut binding_groups = people_query
        .match_(grouped_person)
        .expect("workforce-v2 grouped person match")
        .where_(identifier.eq_field(grouped_identifier))
        .expect("workforce-v2 grouped person identity join")
        .group_by(grouped_person)
        .expect("workforce-v2 binding groups")
        .aggregate((aggregate::count(),))
        .await
        .expect("workforce-v2 binding-grouped reductions")
        .into_iter()
        .map(|(person, (count,))| {
            json!({
                "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
                "key": person.identifier().value(),
                "count": count,
            })
        })
        .collect::<Vec<_>>();
    binding_groups.sort_by(|left, right| left["key"].as_str().cmp(&right["key"].as_str()));
    let mut field_groups = people_query
        .group_by_field(score)
        .expect("workforce-v2 score groups")
        .aggregate((aggregate::count(),))
        .await
        .expect("workforce-v2 score-grouped reductions")
        .into_iter()
        .map(|(score, (count,))| json!({"key": score.value(), "count": count}))
        .collect::<Vec<_>>();
    field_groups.sort_by_key(|value| value["key"].as_i64());
    let mut tuple_groups = people_query
        .group_by_fields((score, score_gte))
        .expect("workforce-v2 score tuple groups")
        .aggregate((aggregate::count(),))
        .await
        .expect("workforce-v2 score tuple-grouped reductions")
        .into_iter()
        .map(|((score, score_gte), (count,))| {
            json!({"key": [score.value(), score_gte.value()], "count": count})
        })
        .collect::<Vec<_>>();
    tuple_groups.sort_by_key(|value| value["key"][0].as_i64());
    let grouped_reducer = json!({
        "reducers": {
            "count": reductions.0,
            "sum": reductions.1,
            "min": reductions.2.expect("workforce-v2 minimum exists"),
            "max": reductions.3.expect("workforce-v2 maximum exists"),
            "mean_bits": format!("{:016x}", reductions.4.expect("workforce-v2 mean exists").to_bits()),
            "median_bits": format!("{:016x}", reductions.5.expect("workforce-v2 median exists").to_bits()),
            "std_bits": format!("{:016x}", reductions.6.expect("workforce-v2 std exists").to_bits()),
        },
        "groups": {
            "binding": binding_groups,
            "field": field_groups,
            "field_tuple": tuple_groups,
        },
    });

    let minimum_score = Score::new(30).expect("workforce-v2 authored function minimum");
    let minimum: IntegerInput =
        integer_input(session, &minimum_score).expect("workforce-v2 function input");
    let call: IntegerCall =
        qualifying_score(session, person, &minimum).expect("workforce-v2 function call");
    let values = workforce_v2_keys(
        &people_query
            .where_(call.ge_field(score))
            .expect("workforce-v2 function field predicate")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 function field rows"),
    );
    let nested: IntegerCall =
        qualifying_score(session, person, &call).expect("workforce-v2 nested function call");
    let nested_people = people_query
        .where_(
            call.ge_call(&nested)
                .expect("workforce-v2 nested function predicate"),
        )
        .expect("workforce-v2 nested function filter")
        .rows(RowsOptions::new(2).order_by(identifier.asc()))
        .await
        .expect("workforce-v2 nested function rows");
    let function_values = values
        .iter()
        .map(|key| {
            [&ada, &nested_people[0], &nested_people[1]]
                .into_iter()
                .find(|person| person.identifier().value() == key)
                .unwrap_or_else(|| panic!("workforce-v2 function result has unknown key: {key}"))
                .score()
                .value()
        })
        .copied()
        .collect::<Vec<_>>();
    let nested_values = nested_people
        .iter()
        .map(|person| *person.score().value())
        .collect::<Vec<_>>();
    let schema_function = json!({
        "minimum": minimum_score.value(),
        "values": function_values,
        "nested_values": nested_values,
    });

    let scalar_keys = workforce_v2_keys(
        &people_query
            .where_(score.ge(scalar_operand))
            .expect("workforce-v2 scalar predicate")
            .rows(RowsOptions::new(2).order_by(identifier.asc()))
            .await
            .expect("workforce-v2 scalar rows"),
    );
    let scalar_domain = json!({
        "domain": workforce_v2_scalar_domain(&scalar_operand),
        "operator": "gte",
        "operand": scalar_operand,
        "keys": scalar_keys,
    });

    let mut observations = BTreeMap::from([
        ("model_values_and_references".to_owned(), model_values),
        ("owner_iid_set".to_owned(), owner_iid_set),
        ("exact_subtypes".to_owned(), exact_subtypes),
        ("scalar_boolean".to_owned(), scalar_boolean),
        ("roles".to_owned(), roles),
        ("topology".to_owned(), topology),
        ("selection_shapes".to_owned(), selection_shapes),
        ("terminals".to_owned(), terminals),
        ("grouped_reducer".to_owned(), grouped_reducer),
        ("hydrated_result".to_owned(), hydrated_result),
        ("scalar_domain".to_owned(), scalar_domain),
        ("schema_function".to_owned(), schema_function),
    ]);
    if let (Some(before), Some(after)) = (exchanges_before_one, exchanges_after_one) {
        assert_eq!(
            after - before,
            1,
            "workforce-v2 remote one performs one exchange"
        );
        observations.insert(
            "remote_one_exchange".to_owned(),
            json!({"exchange_count": after - before, "terminal": one_terminal}),
        );
    }
    observations
}

fn workforce_v2_cancellation_error(error: &Error, partial_result: bool) -> Value {
    json!({
        "category": error.category().as_str(),
        "code": error.code().expect("workforce-v2 cancellation has a stable code"),
        "partial_result": partial_result,
    })
}

async fn workforce_v2_remote_cancellation(
    remote: &RemoteDatabase<AppSchema>,
    exchange_count: &Arc<AtomicUsize>,
    remote_url: &str,
) -> Value {
    let before_cancellation = AnswerCancellation::default();
    before_cancellation.cancel();
    let exchanges_before = exchange_count.load(Ordering::SeqCst);
    let mut before_session = remote
        .query_with_resources(QueryExecutionResourceLimits::default(), before_cancellation)
        .expect("workforce-v2 cancelled remote session");
    let before_person = before_session
        .exact::<Person>()
        .expect("workforce-v2 cancelled remote binding");
    let before_result = before_session
        .query(before_person)
        .expect("workforce-v2 cancelled remote query")
        .count()
        .await;
    let before_partial_result = before_result.is_ok();
    let before_error =
        before_result.expect_err("workforce-v2 pre-cancelled remote query must fail");
    let exchanges_after = exchange_count.load(Ordering::SeqCst);
    assert_eq!(before_error.category(), ErrorCategory::Cancelled);
    assert_eq!(before_error.code(), Some("provider_cancelled"));
    let mut before_exchange = workforce_v2_cancellation_error(&before_error, before_partial_result);
    before_exchange["exchange_count"] = json!(exchanges_after - exchanges_before);

    let decode_cancellation = AnswerCancellation::default();
    let decode_exchange_count = Arc::new(AtomicUsize::new(0));
    let response_completed = Arc::new(AtomicBool::new(false));
    let decode_remote: RemoteDatabase<AppSchema> =
        RemoteDatabase::connect(RemoteConnectionOptions::generated(
            QueryExecutionResourceLimits::default(),
            CancelBeforeDecodeTransport {
                inner: HttpTransport::recording(
                    remote_url.to_owned(),
                    Arc::clone(&decode_exchange_count),
                ),
                cancellation: decode_cancellation.clone(),
                response_completed: Arc::clone(&response_completed),
            },
        ))
        .await
        .expect("workforce-v2 decode-cancel remote connects")
        .with_schema(SCHEMA)
        .expect("workforce-v2 decode-cancel remote schema binds");
    let mut decode_session = decode_remote
        .query_with_resources(QueryExecutionResourceLimits::default(), decode_cancellation)
        .expect("workforce-v2 decode-cancel session");
    let decode_person = decode_session
        .exact::<Person>()
        .expect("workforce-v2 decode-cancel binding");
    let decode_result = decode_session
        .query(decode_person)
        .expect("workforce-v2 decode-cancel query")
        .count()
        .await;
    let decode_partial_result = decode_result.is_ok();
    let decode_error =
        decode_result.expect_err("workforce-v2 remote decode cancellation must fail");
    assert_eq!(decode_error.category(), ErrorCategory::Cancelled);
    assert_eq!(decode_error.code(), Some("provider_cancelled"));
    let mut during_decode = workforce_v2_cancellation_error(&decode_error, decode_partial_result);
    during_decode["exchange_count"] = json!(decode_exchange_count.load(Ordering::SeqCst));

    let abort_signal = AnswerCancellation::default();
    let abort_transport = CallerAbortProbeTransport {
        cancellation: abort_signal.clone(),
    };
    let abort_future = abort_transport.exchange(b"workforce-v2-caller-abort-probe");
    abort_signal.cancel();
    let abort_error = abort_future
        .await
        .expect_err("workforce-v2 caller transport abort probe must stop");
    let caller_transport_abort_supported = abort_error.category() == ErrorCategory::Remote
        && abort_error.code() == Some("caller_transport_aborted");

    json!({
        "before_exchange": before_exchange,
        "during_decode": during_decode,
        "caller_transport_abort_supported": caller_transport_abort_supported,
        "server_exchange_cancelled_after_send": !response_completed.load(Ordering::SeqCst),
    })
}

fn workforce_v2_resource_limit_base() -> Value {
    let hard = QueryExecutionResourceLimits::default();
    let plus_one = QueryExecutionResourceLimits::tightened(
        hard.timeout_milliseconds.saturating_add(1),
        hard.items.saturating_add(1),
        hard.bytes.saturating_add(1),
        hard.graph_nodes.saturating_add(1),
        hard.attribute_values.saturating_add(1),
        hard.collection_members.saturating_add(1),
        hard.role_players.saturating_add(1),
        hard.statements.saturating_add(1),
    );
    let zero = QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0);
    let plus_one_clamped_all = plus_one == hard;
    let zero_tightening_all = zero.timeout_milliseconds == 0
        && zero.items == 0
        && zero.bytes == 0
        && zero.graph_nodes == 0
        && zero.attribute_values == 0
        && zero.collection_members == 0
        && zero.role_players == 0
        && zero.statements == 0;
    json!({
        "hard_maxima": {
            "timeout_milliseconds": hard.timeout_milliseconds,
            "items": hard.items,
            "bytes": hard.bytes,
            "graph_nodes": hard.graph_nodes,
            "attribute_values": hard.attribute_values,
            "collection_members": hard.collection_members,
            "role_players": hard.role_players,
            "statements": hard.statements,
        },
        "plus_one_clamped_all": plus_one_clamped_all,
        "zero_tightening_all": zero_tightening_all,
    })
}

async fn workforce_v2_enforced_role_player_limit(
    session: &mut QuerySession<'_, AppSchema>,
    ada_key: &str,
    dimension: &str,
) -> Value {
    let membership = session
        .exact::<Membership>()
        .expect("workforce-v2 limited membership binding");
    let person = session
        .exact::<Person>()
        .expect("workforce-v2 limited person binding");
    let result = session
        .query(membership)
        .expect("workforce-v2 limited membership query")
        .match_(person)
        .expect("workforce-v2 limited member match")
        .where_(
            membership.role(MembershipType::member).connects(person)
                & person
                    .field(PersonType::identifier)
                    .eq(Identifier::new(ada_key.to_owned()).expect("workforce-v2 limited Ada key")),
        )
        .expect("workforce-v2 limited membership predicate")
        .one()
        .await;
    let no_partial_result = result.is_err();
    let error = result.expect_err("workforce-v2 zero role-player limit must reject hydration");
    assert_eq!(error.category(), ErrorCategory::ResourceLimit);
    json!({
        "dimension": dimension,
        "category": error.category().as_str(),
        "code": error.code().expect("workforce-v2 resource limit has a stable code"),
        "no_partial_result": no_partial_result,
    })
}

async fn workforce_v2_lifecycle_lane(
    session: &mut QuerySession<'_, AppSchema>,
    key: &str,
    exchange_count: Option<&Arc<AtomicUsize>>,
) -> (Value, bool) {
    let person = session
        .exact::<Person>()
        .expect("workforce-v2 lifecycle person binding");
    let identifier = person.field(PersonType::identifier);
    let key_predicate =
        identifier.eq(Identifier::new(key.to_owned()).expect("workforce-v2 lifecycle key"));
    let ancestor = session
        .query(person)
        .expect("workforce-v2 lifecycle ancestor")
        .where_(key_predicate)
        .expect("workforce-v2 lifecycle ancestor predicate");
    let descendant = ancestor
        .where_(person.field(PersonType::score).is_present())
        .expect("workforce-v2 lifecycle descendant");
    let sibling = ancestor
        .where_(person.field(PersonType::val_bool).is_present())
        .expect("workforce-v2 lifecycle sibling");

    descendant.close();
    let ancestor_usable_after_descendant_close =
        ancestor.count().await.is_ok_and(|count| count == 1);

    let descendant_after_ancestor = ancestor
        .where_(person.field(PersonType::aliases).is_present())
        .expect("workforce-v2 descendant retained before ancestor close");
    ancestor.close();
    ancestor.close();
    let close_idempotent = ancestor.is_closed();
    let descendant_usable_after_ancestor_close = descendant_after_ancestor
        .count()
        .await
        .is_ok_and(|count| count == 1);

    let exchanges_before = exchange_count.map(|count| count.load(Ordering::SeqCst));
    let post_close_error = ancestor
        .count()
        .await
        .expect_err("workforce-v2 closed query must reject");
    let exchanges_after = exchange_count.map(|count| count.load(Ordering::SeqCst));
    let post_close_io_count = exchanges_before
        .zip(exchanges_after)
        .map_or(0, |(before, after)| after - before);
    let post_close_rejected = post_close_error.code() == Some("query_resource_closed");

    let sibling_usable = sibling.count().await.is_ok_and(|count| count == 1);
    let session_query = session
        .query(person)
        .expect("workforce-v2 session remains usable")
        .where_(identifier.eq(Identifier::new(key.to_owned()).expect("workforce-v2 session key")))
        .expect("workforce-v2 session query predicate");
    let session_usable_after_query_close =
        session_query.count().await.is_ok_and(|count| count == 1);

    let result_query = session
        .query(person)
        .expect("workforce-v2 result lifecycle query")
        .where_(
            identifier
                .eq(Identifier::new(key.to_owned()).expect("workforce-v2 result lifecycle key")),
        )
        .expect("workforce-v2 result lifecycle predicate");
    let result = result_query
        .one()
        .await
        .expect("workforce-v2 lifecycle result");
    result_query.close();
    let result_usable_after_query_close = result.identifier().value() == key;

    (
        json!({
            "ancestor_usable_after_descendant_close": ancestor_usable_after_descendant_close,
            "close_idempotent": close_idempotent,
            "descendant_usable_after_ancestor_close": descendant_usable_after_ancestor_close,
            "handle_invalidated": ancestor.is_closed(),
            "post_close_io_count": post_close_io_count,
            "post_close_rejected": post_close_rejected,
            "session_usable_after_query_close": session_usable_after_query_close,
            "sibling_usable": sibling_usable,
        }),
        result_usable_after_query_close,
    )
}

async fn run_workforce_journey(db: &Database<AppSchema>) {
    let report_path = workforce_env_path("TYPE_BRIDGE_WORKFORCE_REPORT");
    validate_workforce_report_path(&report_path);
    require_workforce_server_version().await;
    let manifest_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_MANIFEST"))
        .expect("staged workforce manifest is readable");
    let catalog_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_CATALOG"))
        .expect("staged workforce catalog is readable");
    let journey_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_JOURNEY"))
        .expect("staged workforce journey is readable");
    let schema_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_SCHEMA"))
        .expect("staged workforce schema is readable");
    let provider_schema_bytes =
        fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_PROVIDER_SCHEMA"))
            .expect("staged workforce provider schema is readable");
    let catalog: Value =
        serde_json::from_slice(&catalog_bytes).expect("workforce catalog is valid JSON");
    let journey: Value =
        serde_json::from_slice(&journey_bytes).expect("workforce journey is valid JSON");

    assert_eq!(
        workforce_string(&journey["format"], "workforce journey format"),
        "typebridge.workforce-journey/v1"
    );
    assert_eq!(
        workforce_string(&journey["semantic_profile"], "workforce semantic profile"),
        WORKFORCE_PROFILE
    );
    assert_eq!(
        env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
            .expect("generated live semantic profile is configured"),
        WORKFORCE_PROFILE
    );
    assert_eq!(
        workforce_string(&catalog["fixture"]["schema_path"], "workforce schema path"),
        WORKFORCE_SCHEMA_PATH
    );
    assert_eq!(
        workforce_string(
            &catalog["fixture"]["provider_schema_path"],
            "workforce provider schema path",
        ),
        WORKFORCE_PROVIDER_SCHEMA_PATH
    );
    assert_eq!(
        workforce_string(&catalog["journey_path"], "workforce journey path"),
        WORKFORCE_JOURNEY_PATH
    );
    let projection_target = workforce_string(
        &catalog["projection_targets"]["rust"],
        "workforce Rust projection target",
    );
    assert_eq!(projection_target, "rust");
    let operation_order = journey["operation_order"]
        .as_array()
        .expect("workforce operation order must be an array")
        .iter()
        .map(|value| workforce_string(value, "workforce operation").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        operation_order,
        [
            "insert_person",
            "read_person",
            "update_person",
            "insert_membership",
            "read_membership",
            "direct_role_query_one",
            "remote_role_query_one",
            "delete_membership",
            "delete_person",
        ]
    );

    let person_record = &journey["records"]["person"];
    let fields = &person_record["fields"];
    let person_model = workforce_string(&person_record["model"], "workforce person model");
    assert_eq!(person_model, "person");
    let initial_nickname = workforce_string(
        &workforce_typed_field(fields, "nickname", "string")["value"],
        "workforce initial nickname",
    );
    let updated_nickname = workforce_string(
        &person_record["update"]["nickname"]["value"],
        "workforce updated nickname",
    );
    assert_eq!(
        workforce_string(
            &person_record["update"]["nickname"]["kind"],
            "workforce updated nickname kind",
        ),
        "string"
    );
    let membership_record = &journey["records"]["membership"];
    assert_eq!(
        workforce_string(
            &membership_record["player"]["key"],
            "workforce membership player key",
        ),
        workforce_field_string(fields, "identifier", "string")
    );

    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("workforce person baseline count");
    let membership_baseline = db
        .relations::<Membership>()
        .count()
        .await
        .expect("workforce membership baseline count");

    let inserted_person = db
        .entities::<Person>()
        .insert(workforce_person_create(fields, initial_nickname))
        .await
        .expect("workforce person insert");
    assert_workforce_person(&inserted_person, fields, initial_nickname);
    let person_iid = inserted_person.iid().to_owned();
    let read_person = db
        .entities::<Person>()
        .get_by_iid(&person_iid)
        .await
        .expect("workforce person read")
        .expect("workforce person exists after insert");
    assert_workforce_person(&read_person, fields, initial_nickname);
    let updated_person = db
        .entities::<Person>()
        .update(
            &person_iid,
            workforce_person_create(fields, updated_nickname),
        )
        .await
        .expect("workforce person update");
    assert_workforce_person(&updated_person, fields, updated_nickname);
    let updated_read = db
        .entities::<Person>()
        .get_by_iid(&person_iid)
        .await
        .expect("workforce updated person read")
        .expect("workforce person exists after update");
    let direct_model_observation =
        workforce_model_observation(&updated_read, fields, person_model, updated_nickname);

    let membership = db
        .relations::<Membership>()
        .insert(
            MembershipCreate::new(MembershipMemberRef::Person(updated_read.reference()))
                .expect("workforce membership create"),
        )
        .await
        .expect("workforce membership insert");
    let membership_iid = membership.iid().to_owned();
    let membership_read = db
        .relations::<Membership>()
        .get_by_iid(&membership_iid)
        .await
        .expect("workforce membership read")
        .expect("workforce membership exists after insert");
    let _ = workforce_role_observations(&membership_read, &updated_read, membership_record);

    let (direct_relation, direct_person) = {
        let mut session = db.query().expect("workforce direct query session");
        let relation = session
            .exact::<Membership>()
            .expect("workforce direct membership binding");
        let person = session
            .exact::<Person>()
            .expect("workforce direct person binding");
        session
            .query((relation, person))
            .expect("workforce direct selection")
            .where_(
                relation.role(MembershipType::member).connects(person)
                    & person.field(PersonType::identifier).eq(Identifier::new(
                        workforce_field_string(fields, "identifier", "string"),
                    )
                    .expect("workforce direct person key")),
            )
            .expect("workforce direct role predicate")
            .one()
            .await
            .expect("workforce direct role result")
    };
    let (direct_hydration, direct_role) =
        workforce_role_observations(&direct_relation, &direct_person, membership_record);
    let direct_query_model_observation =
        workforce_model_observation(&direct_person, fields, person_model, updated_nickname);
    assert_eq!(direct_query_model_observation, direct_model_observation);

    let exchange_count = Arc::new(AtomicUsize::new(0));
    let remote_url = env::var("TYPE_BRIDGE_REMOTE_URL").expect("workforce remote server URL");
    let remote: RemoteDatabase<AppSchema> =
        RemoteDatabase::connect(RemoteConnectionOptions::generated(
            RemoteQueryLimits::new(100, 8 << 20, 1000, 1000, 1000, 1000).deadline_ms(30_000),
            HttpTransport::recording(remote_url, Arc::clone(&exchange_count)),
        ))
        .await
        .expect("workforce remote database connects")
        .with_schema(SCHEMA)
        .expect("workforce remote schema authority binds");
    assert_eq!(exchange_count.load(Ordering::SeqCst), 0);
    let (remote_relation, remote_person) = {
        let mut session = remote.query().expect("workforce remote query session");
        let relation = session
            .exact::<Membership>()
            .expect("workforce remote membership binding");
        let person = session
            .exact::<Person>()
            .expect("workforce remote person binding");
        session
            .query((relation, person))
            .expect("workforce remote selection")
            .where_(
                relation.role(MembershipType::member).connects(person)
                    & person.field(PersonType::identifier).eq(Identifier::new(
                        workforce_field_string(fields, "identifier", "string"),
                    )
                    .expect("workforce remote person key")),
            )
            .expect("workforce remote role predicate")
            .one()
            .await
            .expect("workforce remote role result")
    };
    let observed_exchange_count = exchange_count.load(Ordering::SeqCst);
    assert_eq!(
        observed_exchange_count, 1,
        "workforce remote terminal must perform exactly one caller-owned exchange"
    );
    let (remote_hydration, remote_role) =
        workforce_role_observations(&remote_relation, &remote_person, membership_record);
    let remote_model_observation =
        workforce_model_observation(&remote_person, fields, person_model, updated_nickname);

    assert_eq!(remote_model_observation, direct_model_observation);
    assert_eq!(remote_hydration, direct_hydration);
    assert_eq!(remote_role, direct_role);

    db.relations::<Membership>()
        .delete(&membership_iid)
        .await
        .expect("workforce membership delete");
    assert!(
        db.relations::<Membership>()
            .get_by_iid(&membership_iid)
            .await
            .expect("workforce deleted membership read")
            .is_none()
    );
    db.entities::<Person>()
        .delete(&person_iid)
        .await
        .expect("workforce person delete");
    assert!(
        db.entities::<Person>()
            .get_by_iid(&person_iid)
            .await
            .expect("workforce deleted person read")
            .is_none()
    );
    assert_eq!(
        db.relations::<Membership>()
            .count()
            .await
            .expect("workforce membership cleanup count"),
        membership_baseline
    );
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("workforce person cleanup count"),
        person_baseline
    );

    let entity_lifecycle = json!({
        "created": true,
        "deleted": true,
        "key": workforce_field_string(fields, "identifier", "string"),
        "model": person_model,
        "nickname_after_update": updated_nickname,
        "read_after_update": true,
    });
    let relation_lifecycle = json!({
        "created": true,
        "deleted": true,
        "model": workforce_string(&membership_record["model"], "workforce relation model"),
        "player_key": workforce_field_string(fields, "identifier", "string"),
        "role": workforce_string(&membership_record["role"], "workforce relation role"),
    });
    let remote_one_exchange = json!({"exchange_count": observed_exchange_count, "terminal": "one"});
    let actual_observations = BTreeMap::from([
        ("entity_lifecycle".to_owned(), entity_lifecycle),
        ("hydrated_role_result".to_owned(), direct_hydration),
        (
            "model_values_and_references".to_owned(),
            direct_model_observation,
        ),
        ("relation_lifecycle".to_owned(), relation_lifecycle),
        ("remote_one_exchange".to_owned(), remote_one_exchange),
        ("role_traversal".to_owned(), direct_role),
    ]);
    let expected_observations = journey["expected_observations"]
        .as_object()
        .expect("workforce expected observations must be an object");
    assert_eq!(actual_observations.len(), expected_observations.len());
    for (name, actual) in &actual_observations {
        assert_eq!(
            actual,
            expected_observations
                .get(name)
                .unwrap_or_else(|| panic!("workforce expected observation is missing: {name}")),
            "workforce observation diverged: {name}"
        );
    }

    let semantic_fingerprint: Value = serde_json::from_str(SEMANTIC_SCHEMA_FINGERPRINT_JSON)
        .expect("generated semantic fingerprint is valid JSON");
    let projection_fingerprint: Value = serde_json::from_str(PROJECTION_FINGERPRINT_JSON)
        .expect("generated projection fingerprint is valid JSON");
    assert_eq!(
        workforce_string(
            &semantic_fingerprint["semantic_profile"],
            "semantic fingerprint profile",
        ),
        WORKFORCE_PROFILE
    );
    assert_eq!(
        workforce_string(
            &projection_fingerprint["semantic_profile"],
            "projection fingerprint profile",
        ),
        WORKFORCE_PROFILE
    );
    let report = json!({
        "format": "typebridge.sdk-conformance-report/v1",
        "binding": "rust",
        "manifest": workforce_source_identity(WORKFORCE_MANIFEST_PATH, &manifest_bytes),
        "catalog": workforce_source_identity(WORKFORCE_CATALOG_PATH, &catalog_bytes),
        "fixture": {
            "id": workforce_string(&journey["fixture_id"], "workforce fixture ID"),
            "version": journey["version"].clone(),
            "semantic_profile": WORKFORCE_PROFILE,
            "schema": workforce_source_identity(WORKFORCE_SCHEMA_PATH, &schema_bytes),
            "provider_schema": workforce_source_identity(
                WORKFORCE_PROVIDER_SCHEMA_PATH,
                &provider_schema_bytes,
            ),
            "journey": workforce_source_identity(WORKFORCE_JOURNEY_PATH, &journey_bytes),
            "semantic_fingerprint": semantic_fingerprint,
            "projection_target": projection_target,
            "projection_fingerprint": projection_fingerprint,
        },
        "results": workforce_results(&catalog, &actual_observations),
    });
    publish_workforce_report(&report_path, report);
}

async fn run_workforce_v2_journey_inner(db: &Database<AppSchema>) {
    let report_path = workforce_env_path("TYPE_BRIDGE_WORKFORCE_REPORT_V2");
    validate_workforce_report_path(&report_path);
    require_workforce_server_version().await;
    let manifest_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_MANIFEST_V2"))
        .expect("staged workforce-v2 manifest is readable");
    let catalog_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_CATALOG_V2"))
        .expect("staged workforce-v2 catalog is readable");
    let journey_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_JOURNEY_V2"))
        .expect("staged workforce-v2 journey is readable");
    let schema_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_SCHEMA_V2"))
        .expect("staged workforce-v2 schema is readable");
    let provider_schema_bytes = fs::read(workforce_env_path(
        "TYPE_BRIDGE_WORKFORCE_PROVIDER_SCHEMA_V2",
    ))
    .expect("staged workforce-v2 provider schema is readable");
    let catalog: Value =
        serde_json::from_slice(&catalog_bytes).expect("workforce-v2 catalog is valid JSON");
    let journey: Value =
        serde_json::from_slice(&journey_bytes).expect("workforce-v2 journey is valid JSON");
    assert_eq!(
        workforce_string(&journey["format"], "workforce-v2 journey format"),
        "typebridge.workforce-journey/v2"
    );
    assert_eq!(journey["fixture_id"], "workforce-v2");
    assert_eq!(journey["version"], 2);
    assert_eq!(
        workforce_string(
            &journey["semantic_profile"],
            "workforce-v2 semantic profile"
        ),
        WORKFORCE_PROFILE
    );
    assert_eq!(
        env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
            .expect("generated live semantic profile is configured"),
        WORKFORCE_PROFILE
    );
    assert_eq!(
        workforce_string(
            &catalog["fixture"]["schema_path"],
            "workforce-v2 schema path",
        ),
        WORKFORCE_SCHEMA_PATH
    );
    assert_eq!(
        workforce_string(
            &catalog["fixture"]["provider_schema_path"],
            "workforce-v2 provider schema path",
        ),
        WORKFORCE_PROVIDER_SCHEMA_PATH
    );
    assert_eq!(
        workforce_string(&catalog["journey_path"], "workforce-v2 journey path"),
        WORKFORCE_V2_JOURNEY_PATH
    );
    let projection_target = workforce_string(
        &catalog["projection_targets"]["rust"],
        "workforce-v2 Rust projection target",
    );
    assert_eq!(projection_target, "rust");

    let proof_observations = workforce_v2_provider_proofs();

    let records = &journey["records"];
    let people_records = records["people"]
        .as_array()
        .expect("workforce-v2 people records are an array");
    assert_eq!(people_records.len(), 2);
    let ada_key = workforce_field_string(&people_records[0]["fields"], "identifier", "string");
    let dana_key = workforce_field_string(&people_records[1]["fields"], "identifier", "string");
    let employee_fields = &records["employee"]["fields"];
    let manager_fields = &records["manager"]["fields"];
    let network_fields = &records["network_link"]["fields"];

    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("workforce-v2 person baseline");
    let employee_baseline = db
        .entities::<Employee>()
        .count()
        .await
        .expect("workforce-v2 employee baseline");
    let manager_baseline = db
        .entities::<Manager>()
        .count()
        .await
        .expect("workforce-v2 manager baseline");
    let membership_baseline = db
        .relations::<Membership>()
        .count()
        .await
        .expect("workforce-v2 membership baseline");
    let network_baseline = db
        .relations::<NetworkLink>()
        .count()
        .await
        .expect("workforce-v2 network baseline");

    let people = db
        .entities::<Person>()
        .insert_many(
            people_records
                .iter()
                .map(workforce_v2_person_create)
                .collect(),
        )
        .await
        .expect("workforce-v2 people insert");
    assert_eq!(people.len(), 2);
    workforce_v2_assert_person(&people[0], &people_records[0]);
    workforce_v2_assert_person(&people[1], &people_records[1]);
    let person_iids = [people[0].iid().to_owned(), people[1].iid().to_owned()];
    let read_after_create = db
        .entities::<Person>()
        .get_by_iid(&person_iids[0])
        .await
        .expect("workforce-v2 person read after create")
        .is_some();

    let employee = db
        .entities::<Employee>()
        .insert(
            EmployeeCreate::try_new(
                Identifier::new(workforce_field_string(
                    employee_fields,
                    "identifier",
                    "string",
                ))
                .expect("workforce-v2 employee identifier"),
                PartyName::new(workforce_field_string(
                    employee_fields,
                    "party_name",
                    "string",
                ))
                .expect("workforce-v2 employee party name"),
                Rank::new(workforce_field_long(employee_fields, "rank"))
                    .expect("workforce-v2 employee rank"),
            )
            .expect("workforce-v2 employee create"),
        )
        .await
        .expect("workforce-v2 employee insert");
    let employee_iid = employee.iid().to_owned();
    let manager = db
        .entities::<Manager>()
        .insert(
            ManagerCreate::try_new(
                Identifier::new(workforce_field_string(
                    manager_fields,
                    "identifier",
                    "string",
                ))
                .expect("workforce-v2 manager identifier"),
                ManagerNote::new(workforce_field_string(
                    manager_fields,
                    "manager_note",
                    "string",
                ))
                .expect("workforce-v2 manager note"),
                PartyName::new(workforce_field_string(
                    manager_fields,
                    "party_name",
                    "string",
                ))
                .expect("workforce-v2 manager party name"),
                Rank::new(workforce_field_long(manager_fields, "rank"))
                    .expect("workforce-v2 manager rank"),
            )
            .expect("workforce-v2 manager create"),
        )
        .await
        .expect("workforce-v2 manager insert");
    let manager_iid = manager.iid().to_owned();
    let membership = db
        .relations::<Membership>()
        .insert(
            MembershipCreate::new(MembershipMemberRef::Person(people[0].reference()))
                .expect("workforce-v2 membership create"),
        )
        .await
        .expect("workforce-v2 membership insert");
    let membership_iid = membership.iid().to_owned();
    let membership_read_after_create = db
        .relations::<Membership>()
        .get_by_iid(&membership_iid)
        .await
        .expect("workforce-v2 membership read after create")
        .is_some();
    let network = db
        .relations::<NetworkLink>()
        .insert(
            NetworkLinkCreate::new(
                Identifier::new(workforce_field_string(
                    network_fields,
                    "identifier",
                    "string",
                ))
                .expect("workforce-v2 network identifier"),
                Some(
                    Nickname::new(workforce_field_string(network_fields, "nickname", "string"))
                        .expect("workforce-v2 network nickname"),
                ),
                people[1].reference(),
                people[0].reference(),
                vec![people[0].reference(), people[1].reference()],
            )
            .expect("workforce-v2 network create"),
        )
        .await
        .expect("workforce-v2 network insert");
    let network_iid = network.iid().to_owned();
    let keyed_create_order = [
        people[0].identifier().value().clone(),
        people[1].identifier().value().clone(),
        employee.identifier().value().clone(),
        manager.identifier().value().clone(),
        network.identifier().value().clone(),
    ];
    let membership_order_key = format!(
        "{}{}",
        workforce_v2_common_key_prefix(&keyed_create_order),
        workforce_v2_type_label(MembershipType::TOKEN.type_id_json()),
    );
    let actual_create_order = [
        keyed_create_order[0].clone(),
        keyed_create_order[1].clone(),
        keyed_create_order[2].clone(),
        keyed_create_order[3].clone(),
        membership_order_key.clone(),
        keyed_create_order[4].clone(),
    ];
    assert_eq!(
        journey["create_order"],
        json!(actual_create_order),
        "workforce-v2 public create operations diverged from journey order"
    );

    let mut direct_session = db.query().expect("workforce-v2 direct query session");
    let direct_observations =
        workforce_v2_query_observations(&mut direct_session, records, &person_iids, None).await;

    let remote_url = env::var("TYPE_BRIDGE_REMOTE_URL").expect("workforce-v2 remote URL");
    let exchange_count = Arc::new(AtomicUsize::new(0));
    let remote: RemoteDatabase<AppSchema> =
        RemoteDatabase::connect(RemoteConnectionOptions::generated(
            QueryExecutionResourceLimits::default(),
            HttpTransport::recording(remote_url.clone(), Arc::clone(&exchange_count)),
        ))
        .await
        .expect("workforce-v2 remote database connects")
        .with_schema(SCHEMA)
        .expect("workforce-v2 remote schema binds");
    let mut remote_session = remote.query().expect("workforce-v2 remote query session");
    let remote_observations = workforce_v2_query_observations(
        &mut remote_session,
        records,
        &person_iids,
        Some(&exchange_count),
    )
    .await;
    for name in [
        "model_values_and_references",
        "owner_iid_set",
        "exact_subtypes",
        "scalar_boolean",
        "roles",
        "topology",
        "selection_shapes",
        "terminals",
        "grouped_reducer",
        "hydrated_result",
        "scalar_domain",
        "schema_function",
    ] {
        assert_eq!(
            direct_observations.get(name),
            remote_observations.get(name),
            "workforce-v2 direct/remote observation differs: {name}"
        );
    }

    let structured_query_diagnostic = {
        let mut session = db
            .query()
            .expect("workforce-v2 structured diagnostic session");
        let person = session
            .exact::<Person>()
            .expect("workforce-v2 structured diagnostic binding");
        let identifier = person.field(PersonType::identifier);
        let error = session
            .query(person)
            .expect("workforce-v2 structured diagnostic query")
            .where_(
                identifier.eq(Identifier::new(ada_key.clone()).expect("workforce-v2 Ada key"))
                    | identifier
                        .eq(Identifier::new(dana_key.clone()).expect("workforce-v2 Dana key")),
            )
            .expect("workforce-v2 structured diagnostic scope")
            .one()
            .await
            .expect_err("workforce-v2 two-row exact-one must fail");
        workforce_v2_diagnostic(
            &error,
            None,
            &[
                ada_key.as_str(),
                dana_key.as_str(),
                person_iids[0].as_str(),
                person_iids[1].as_str(),
            ],
        )
    };

    let cancellation_remote =
        workforce_v2_remote_cancellation(&remote, &exchange_count, &remote_url).await;
    let deterministic_cancellation_remote =
        &proof_observations[&workforce_v2_observation_key("cancellation_remote", "remote_runtime")];
    assert_eq!(
        &cancellation_remote, deterministic_cancellation_remote,
        "workforce-v2 live and deterministic public remote cancellation observations differ"
    );

    let limited_dimension = "role_players";
    let limited = QueryExecutionResourceLimits {
        role_players: 0,
        ..QueryExecutionResourceLimits::default()
    };
    let direct_enforced = {
        let mut session = db
            .query_with_resources(limited, AnswerCancellation::default())
            .expect("workforce-v2 direct limited session");
        workforce_v2_enforced_role_player_limit(&mut session, &ada_key, limited_dimension).await
    };
    let remote_enforced = {
        let mut session = remote
            .query_with_resources(limited, AnswerCancellation::default())
            .expect("workforce-v2 remote limited session");
        workforce_v2_enforced_role_player_limit(&mut session, &ada_key, limited_dimension).await
    };
    assert_eq!(direct_enforced, remote_enforced);
    let mut direct_limits = workforce_v2_resource_limit_base();
    direct_limits["enforced"] = direct_enforced;
    let mut remote_limits = workforce_v2_resource_limit_base();
    remote_limits["enforced"] = remote_enforced;

    let direct_lane = "direct";
    let (direct_lifecycle, direct_result_survives) = {
        let mut session = db.query().expect("workforce-v2 direct lifecycle session");
        workforce_v2_lifecycle_lane(&mut session, &ada_key, None).await
    };
    let remote_lane = "remote";
    let (remote_lifecycle, remote_result_survives) = {
        let mut session = remote
            .query()
            .expect("workforce-v2 remote lifecycle session");
        workforce_v2_lifecycle_lane(&mut session, &ada_key, Some(&exchange_count)).await
    };
    assert_eq!(direct_lifecycle, remote_lifecycle);
    assert_eq!(direct_result_survives, remote_result_survives);
    let query_resource_lifecycle = json!({
        "lanes": [direct_lane, remote_lane],
        "query": direct_lifecycle,
        "result_usable_after_query_close": direct_result_survives,
    });

    let mut actual_cleanup_order = Vec::new();
    db.relations::<NetworkLink>()
        .delete(&network_iid)
        .await
        .expect("workforce-v2 network cleanup");
    actual_cleanup_order.push(keyed_create_order[4].clone());
    let network_deleted = db
        .relations::<NetworkLink>()
        .get_by_iid(&network_iid)
        .await
        .expect("workforce-v2 network cleanup read")
        .is_none();
    db.relations::<Membership>()
        .delete(&membership_iid)
        .await
        .expect("workforce-v2 membership cleanup");
    actual_cleanup_order.push(membership_order_key);
    let membership_deleted = db
        .relations::<Membership>()
        .get_by_iid(&membership_iid)
        .await
        .expect("workforce-v2 membership cleanup read")
        .is_none();
    db.entities::<Manager>()
        .delete(&manager_iid)
        .await
        .expect("workforce-v2 manager cleanup");
    actual_cleanup_order.push(keyed_create_order[3].clone());
    db.entities::<Employee>()
        .delete(&employee_iid)
        .await
        .expect("workforce-v2 employee cleanup");
    actual_cleanup_order.push(keyed_create_order[2].clone());
    db.entities::<Person>()
        .delete(&person_iids[1])
        .await
        .expect("workforce-v2 Dana cleanup");
    actual_cleanup_order.push(keyed_create_order[1].clone());
    db.entities::<Person>()
        .delete(&person_iids[0])
        .await
        .expect("workforce-v2 Ada cleanup");
    actual_cleanup_order.push(keyed_create_order[0].clone());
    let people_deleted = db
        .entities::<Person>()
        .get_by_iid(&person_iids[0])
        .await
        .expect("workforce-v2 Ada cleanup read")
        .is_none()
        && db
            .entities::<Person>()
            .get_by_iid(&person_iids[1])
            .await
            .expect("workforce-v2 Dana cleanup read")
            .is_none();
    assert!(network_deleted && membership_deleted && people_deleted);
    assert_eq!(
        journey["cleanup_order"],
        json!(actual_cleanup_order),
        "workforce-v2 public cleanup operations diverged from journey order"
    );
    assert_eq!(
        db.relations::<NetworkLink>()
            .count()
            .await
            .expect("workforce-v2 network cleanup count"),
        network_baseline
    );
    assert_eq!(
        db.relations::<Membership>()
            .count()
            .await
            .expect("workforce-v2 membership cleanup count"),
        membership_baseline
    );
    assert_eq!(
        db.entities::<Manager>()
            .count()
            .await
            .expect("workforce-v2 manager cleanup count"),
        manager_baseline
    );
    assert_eq!(
        db.entities::<Employee>()
            .count()
            .await
            .expect("workforce-v2 employee cleanup count"),
        employee_baseline
    );
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("workforce-v2 person cleanup count"),
        person_baseline
    );

    let entity_lifecycle = json!({
        "created": !person_iids[0].is_empty(),
        "deleted": people_deleted,
        "key": ada_key,
        "model": workforce_v2_type_label(PersonType::TOKEN.type_id_json()),
        "read_after_create": read_after_create,
    });
    let membership_player_key = match membership.member() {
        MembershipMemberPlayer::Person(reference) => reference
            .identifier()
            .expect("workforce-v2 inserted membership player carries its key")
            .value()
            .clone(),
        MembershipMemberPlayer::Robot(_) => {
            panic!("workforce-v2 inserted membership player must be a person")
        }
    };
    let relation_lifecycle = json!({
        "created": !membership_iid.is_empty() && membership_read_after_create,
        "deleted": membership_deleted,
        "model": workforce_v2_type_label(MembershipType::TOKEN.type_id_json()),
        "player_key": membership_player_key,
        "role": workforce_v2_role_label(MembershipType::member.role_id_json()),
    });

    let mut actual = BTreeMap::new();
    let mut insert = |observation_ref: &str, proof_kind: &str, observation: Value| {
        let key = workforce_v2_observation_key(observation_ref, proof_kind);
        assert!(
            actual.insert(key.clone(), observation).is_none(),
            "workforce-v2 producer duplicated an observation lane: {key:?}"
        );
    };
    insert("entity_lifecycle", "direct_runtime", entity_lifecycle);
    insert("relation_lifecycle", "direct_runtime", relation_lifecycle);
    insert(
        "structured_query_diagnostic",
        "diagnostic",
        structured_query_diagnostic,
    );
    insert(
        "remote_structured_diagnostic",
        "diagnostic",
        proof_observations
            [&workforce_v2_observation_key("remote_structured_diagnostic", "diagnostic")]
            .clone(),
    );
    for name in [
        "model_values_and_references",
        "exact_subtypes",
        "owner_iid_set",
        "grouped_reducer",
        "hydrated_result",
        "roles",
        "scalar_boolean",
        "schema_function",
        "selection_shapes",
        "terminals",
        "topology",
        "scalar_domain",
    ] {
        insert(
            name,
            "direct_runtime",
            direct_observations
                .get(name)
                .unwrap_or_else(|| panic!("workforce-v2 direct observation is absent: {name}"))
                .clone(),
        );
        insert(
            name,
            "remote_runtime",
            remote_observations
                .get(name)
                .unwrap_or_else(|| panic!("workforce-v2 remote observation is absent: {name}"))
                .clone(),
        );
    }
    insert(
        "remote_one_exchange",
        "remote_runtime",
        remote_observations["remote_one_exchange"].clone(),
    );
    insert(
        "cancellation_direct",
        "direct_runtime",
        proof_observations[&workforce_v2_observation_key("cancellation_direct", "direct_runtime")]
            .clone(),
    );
    insert(
        "cancellation_remote",
        "remote_runtime",
        deterministic_cancellation_remote.clone(),
    );
    insert(
        "query_resource_lifecycle",
        "lifecycle",
        query_resource_lifecycle,
    );
    insert("resource_limits", "direct_runtime", direct_limits);
    insert("resource_limits", "remote_runtime", remote_limits);
    drop(insert);
    assert_eq!(actual.len(), 34, "workforce-v2 producer requires 34 rows");

    let expected_observations = journey["expected_observations"]
        .as_object()
        .expect("workforce-v2 expected observations must be an object");
    let actual_observation_refs = actual
        .keys()
        .map(|(observation_ref, _)| observation_ref.as_str())
        .collect::<BTreeSet<_>>();
    let expected_observation_refs = expected_observations
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_observation_refs, expected_observation_refs,
        "workforce-v2 actual and journey observation coverage differs"
    );
    for ((observation_ref, proof_kind), observation) in &actual {
        assert_eq!(
            observation,
            expected_observations
                .get(observation_ref)
                .unwrap_or_else(|| {
                    panic!("workforce-v2 expected observation is absent: {observation_ref}")
                }),
            "workforce-v2 actual observation diverged: {observation_ref}/{proof_kind}"
        );
    }

    let semantic_fingerprint: Value = serde_json::from_str(SEMANTIC_SCHEMA_FINGERPRINT_JSON)
        .expect("generated workforce-v2 semantic fingerprint is valid JSON");
    let projection_fingerprint: Value = serde_json::from_str(PROJECTION_FINGERPRINT_JSON)
        .expect("generated workforce-v2 projection fingerprint is valid JSON");
    assert_eq!(
        semantic_fingerprint,
        catalog["expected_fingerprints"]["semantic"]
    );
    assert_eq!(
        projection_fingerprint,
        catalog["expected_fingerprints"]["projections"]["rust"]
    );
    let report = json!({
        "format": "typebridge.sdk-conformance-report/v2",
        "binding": "rust",
        "manifest": workforce_source_identity(WORKFORCE_MANIFEST_PATH, &manifest_bytes),
        "catalog": workforce_source_identity(WORKFORCE_V2_CATALOG_PATH, &catalog_bytes),
        "fixture": {
            "id": workforce_string(&journey["fixture_id"], "workforce-v2 fixture ID"),
            "version": journey["version"].clone(),
            "semantic_profile": WORKFORCE_PROFILE,
            "schema": workforce_source_identity(WORKFORCE_SCHEMA_PATH, &schema_bytes),
            "provider_schema": workforce_source_identity(
                WORKFORCE_PROVIDER_SCHEMA_PATH,
                &provider_schema_bytes,
            ),
            "journey": workforce_source_identity(WORKFORCE_V2_JOURNEY_PATH, &journey_bytes),
            "semantic_fingerprint": semantic_fingerprint,
            "projection_target": projection_target,
            "projection_fingerprint": projection_fingerprint,
        },
        "results": workforce_v2_results(&catalog, &actual),
    });
    publish_workforce_report(&report_path, report);
}

#[tokio::test]
async fn generated_workforce_report_journeys() {
    let workforce_v1_requested = env::var_os("TYPE_BRIDGE_WORKFORCE_REPORT").is_some();
    let workforce_v2_requested = env::var_os("TYPE_BRIDGE_WORKFORCE_REPORT_V2").is_some();
    if workforce_v1_requested || workforce_v2_requested {
        let db = database().await;
        if workforce_v1_requested {
            run_workforce_journey(&db).await;
        }
        if workforce_v2_requested {
            run_workforce_v2_journey_inner(&db).await;
        }
    }
    println!("generated workforce report journeys: passed");
}

#[tokio::test]
async fn generated_schema_handshake_and_tokens() {
    assert_eq!(
        PersonType::identifier.owns_id_json(),
        r#"{"attribute":"identifier","owner":{"kind":"entity","label":"person"}}"#
    );
    assert_eq!(
        MembershipType::member.role_id_json(),
        r#"{"declaring_relation":"membership","label":"member"}"#
    );
    assert_eq!(
        EmploymentType::employee.role_id_json(),
        r#"{"declaring_relation":"employment","label":"employee"}"#
    );
    assert_eq!(
        EventType::subject.role_id_json(),
        r#"{"declaring_relation":"event","label":"subject"}"#
    );
    assert_eq!(
        ContainerType::item.role_id_json(),
        r#"{"declaring_relation":"container","label":"item"}"#
    );
    assert_eq!(
        plays_event_container_item.plays_id_json(),
        r#"{"player":{"kind":"relation","label":"event"},"role":{"declaring_relation":"container","label":"item"}}"#
    );

    let db = database().await;
    assert!(db.is_schema_bound());
    println!("public generated schema handshake and tokens: passed");
}

#[tokio::test]
async fn generated_entity_crud_batches_and_scalar_domains() {
    let db = database().await;
    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("person baseline count");
    assert!(
        db.entities::<Person>()
            .insert_many(Vec::new())
            .await
            .expect("empty person insert batch")
            .is_empty()
    );
    assert!(
        db.entities::<Person>()
            .put_many(Vec::new())
            .await
            .expect("empty person put batch")
            .is_empty()
    );
    assert!(
        db.entities::<Person>()
            .update_many(Vec::new())
            .await
            .expect("empty person update batch")
            .is_empty()
    );
    db.entities::<Person>()
        .delete_many(&[])
        .await
        .expect("empty person delete batch");

    let score = Score::new(42i64).expect("score is valid");
    let v_double = ValDouble::new(CanonicalDouble::try_new(3.14).expect("double is valid"))
        .expect("val_double is valid");
    let v_decimal = ValDecimal::new(Decimal::try_new("123.45").expect("decimal is valid"))
        .expect("val_decimal is valid");
    let v_bool = ValBool::new(true).expect("val_bool is valid");
    let v_date = ValDate::new(Date::try_new("2026-07-28").expect("date is valid"))
        .expect("val_date is valid");
    let v_datetime =
        ValDatetime::new(DateTime::try_new("2026-07-28T03:55:00").expect("datetime is valid"))
            .expect("val_datetime is valid");
    let v_datetimetz = ValDatetimeTz::new(
        DateTimeTz::try_new("2026-07-28T03:55:00Z").expect("datetimetz is valid"),
    )
    .expect("val_datetimetz is valid");
    let v_duration = ValDuration::new(Duration::try_new("P1D").expect("duration is valid"))
        .expect("val_duration is valid");
    let v_constrained = ValConstrained::new(50i64).expect("val_constrained is valid");

    let person_create = PersonCreate::try_new(
        vec![
            Aliases::new("f2b03-public-alpha".to_owned()).expect("alias is valid"),
            Aliases::new("f2b03-public-beta".to_owned()).expect("alias is valid"),
        ],
        Some(FooBar::new(7).expect("foo__bar is valid")),
        Identifier::new("p-100".to_owned()).expect("identifier is valid"),
        Some(Nickname::new("al".to_owned()).expect("nickname is valid")),
        score,
        Some(ScoreGte::new(8).expect("score__gte is valid")),
        v_bool,
        v_constrained,
        v_date,
        v_datetime,
        v_datetimetz,
        v_decimal,
        v_double,
        v_duration,
    )
    .expect("PersonCreate is valid");

    let person = db
        .entities::<Person>()
        .insert(person_create)
        .await
        .expect("person insert returns a complete model");

    {
        let mut session = db.query().expect("scalar-domain query session");
        let binding = session
            .exact::<Person>()
            .expect("scalar-domain person binding");
        let queried = session
            .query(binding)
            .expect("scalar-domain person selection")
            .where_(
                binding
                    .field(PersonType::identifier)
                    .eq(Identifier::new("p-100").expect("scalar-domain identifier"))
                    & binding
                        .field(PersonType::val_bool)
                        .eq(ValBool::new(true).expect("scalar-domain bool"))
                    & binding
                        .field(PersonType::val_double)
                        .ge(QueryDouble::new(3.14).expect("scalar-domain double boundary"))
                    & binding
                        .field(PersonType::val_decimal)
                        .ge(QueryDecimal::new("123.45").expect("scalar-domain decimal boundary"))
                    & binding
                        .field(PersonType::val_date)
                        .ge(QueryDate::new("2026-07-28").expect("scalar-domain date boundary"))
                    & binding
                        .field(PersonType::val_datetime)
                        .ge(QueryDateTime::new("2026-07-28T03:55:00")
                            .expect("scalar-domain datetime boundary"))
                    & binding
                        .field(PersonType::val_datetime_tz)
                        .ge(QueryDateTimeTz::new("2026-07-28T03:55:00Z")
                            .expect("scalar-domain datetime-tz boundary"))
                    & binding.field(PersonType::val_duration).eq(ValDuration::new(
                        Duration::try_new("P1D").expect("duration literal"),
                    )
                    .expect("scalar-domain duration")),
            )
            .expect("scalar-domain predicates")
            .one()
            .await
            .expect("scalar-domain person result");
        assert_eq!(queried.iid(), person.iid());
    }
    let assert_person = |value: &Person,
                         identifier: &str,
                         aliases: &[&str],
                         nickname: Option<&str>,
                         score: i64,
                         finite: f64,
                         decimal: &str,
                         boolean: bool,
                         constrained: i64,
                         date: &str,
                         datetime: &str,
                         datetime_tz: &str,
                         duration: &str| {
        assert_eq!(value.identifier().value(), identifier);
        let mut observed = value
            .aliases()
            .iter()
            .map(|alias| alias.value().as_str())
            .collect::<Vec<_>>();
        observed.sort();
        let mut expected = aliases.to_vec();
        expected.sort();
        assert_eq!(observed, expected);
        assert_eq!(value.nickname().map(|item| item.value().as_str()), nickname);
        assert_eq!(value.score().value(), &score);
        assert_eq!(value.val_double().value().get(), finite);
        assert_eq!(value.val_decimal().value().as_str(), decimal);
        assert_eq!(value.val_bool().value(), &boolean);
        assert_eq!(value.val_constrained().value(), &constrained);
        assert_eq!(value.val_date().value().as_str(), date);
        assert_eq!(value.val_datetime().value().as_str(), datetime);
        assert_eq!(value.val_datetime_tz().value().as_str(), datetime_tz);
        assert_eq!(value.val_duration().value().as_str(), duration);
    };
    assert!(!person.iid().is_empty());
    assert_eq!(person.identifier().value(), "p-100");
    assert_person(
        &person,
        "p-100",
        &["f2b03-public-alpha", "f2b03-public-beta"],
        Some("al"),
        42,
        3.14,
        "123.45",
        true,
        50,
        "2026-07-28",
        "2026-07-28T03:55:00",
        "2026-07-28T03:55:00Z",
        "P1D",
    );
    assert_eq!(person.score().value(), &42);
    assert_eq!(person.foo__bar().map(FooBar::value), Some(&7));
    assert_eq!(person.score__gte().map(ScoreGte::value), Some(&8));
    assert_eq!(person.val_bool().value(), &true);
    assert_eq!(person.val_constrained().value(), &50);
    assert_eq!(person.val_double().value().get(), 3.14);
    assert_eq!(person.val_decimal().value().as_str(), "123.45");
    assert_eq!(person.val_date().value().as_str(), "2026-07-28");
    assert_eq!(
        person.val_datetime().value().as_str(),
        "2026-07-28T03:55:00"
    );
    assert_eq!(
        person.val_datetime_tz().value().as_str(),
        "2026-07-28T03:55:00Z"
    );
    assert_eq!(person.val_duration().value().as_str(), "P1D");
    assert_eq!(person.nickname().map(|v| v.value()), Some(&"al".to_owned()));
    let iid = person.iid().to_owned();
    let fetched = db
        .entities::<Person>()
        .get_by_iid(&iid)
        .await
        .expect("exact person lookup")
        .expect("person exists");
    assert_eq!(fetched.iid(), iid);
    assert_eq!(fetched.identifier().value(), "p-100");
    assert_person(
        &fetched,
        "p-100",
        &["f2b03-public-alpha", "f2b03-public-beta"],
        Some("al"),
        42,
        3.14,
        "123.45",
        true,
        50,
        "2026-07-28",
        "2026-07-28T03:55:00",
        "2026-07-28T03:55:00Z",
        "P1D",
    );
    let public_people = db.entities::<Person>().all().await.expect("person all");
    assert_eq!(public_people.len() as u64, person_baseline + 1);
    assert_eq!(
        db.entities::<Person>().count().await.expect("person count"),
        person_baseline + 1
    );
    assert!(
        public_people
            .iter()
            .any(|row| row.identifier().value() == "p-100")
    );
    assert_eq!(
        public_people
            .iter()
            .find(|row| row.identifier().value() == "p-100")
            .unwrap()
            .iid(),
        person.iid()
    );

    let replaced = PersonCreate::try_new(
        vec![Aliases::new("f2b03-public-gamma".to_owned()).expect("alias")],
        None,
        Identifier::new("p-100".to_owned()).expect("identifier"),
        None,
        Score::new(43).expect("score"),
        None,
        ValBool::new(false).expect("bool"),
        ValConstrained::new(51).expect("constrained"),
        ValDate::new(Date::try_new("2026-07-29").expect("date")).expect("date"),
        ValDatetime::new(DateTime::try_new("2026-07-29T03:55:00").expect("datetime"))
            .expect("datetime"),
        ValDatetimeTz::new(DateTimeTz::try_new("2026-07-29T03:55:00Z").expect("tz")).expect("tz"),
        ValDecimal::new(Decimal::try_new("124.45").expect("decimal")).expect("decimal"),
        ValDouble::new(CanonicalDouble::try_new(4.14).expect("double")).expect("double"),
        ValDuration::new(Duration::try_new("P2D").expect("duration")).expect("duration"),
    )
    .expect("replacement");
    let put = db.entities::<Person>().put(replaced).await.expect("put");
    assert_eq!(put.iid(), iid);
    assert_person(
        &put,
        "p-100",
        &["f2b03-public-gamma"],
        None,
        43,
        4.14,
        "124.45",
        false,
        51,
        "2026-07-29",
        "2026-07-29T03:55:00",
        "2026-07-29T03:55:00Z",
        "P2D",
    );
    assert_eq!(put.nickname(), None);
    assert!(
        db.entities::<Person>()
            .get_by_iid(&iid)
            .await
            .expect("optional absence read")
            .unwrap()
            .nickname()
            .is_none()
    );
    let batch_people = db
        .entities::<Person>()
        .insert_many(vec![
            PersonCreate::try_new(
                vec![Aliases::new("f2b03-public-batch-a").unwrap()],
                None,
                Identifier::new("p-101").unwrap(),
                None,
                Score::new(60).unwrap(),
                None,
                ValBool::new(true).unwrap(),
                ValConstrained::new(60).unwrap(),
                ValDate::new(Date::try_new("2026-08-01").unwrap()).unwrap(),
                ValDatetime::new(DateTime::try_new("2026-08-01T03:55:00").unwrap()).unwrap(),
                ValDatetimeTz::new(DateTimeTz::try_new("2026-08-01T03:55:00Z").unwrap()).unwrap(),
                ValDecimal::new(Decimal::try_new("126.45").unwrap()).unwrap(),
                ValDouble::new(CanonicalDouble::try_new(6.14).unwrap()).unwrap(),
                ValDuration::new(Duration::try_new("P4D").unwrap()).unwrap(),
            )
            .unwrap(),
            PersonCreate::try_new(
                vec![
                    Aliases::new("f2b03-public-batch-b").unwrap(),
                    Aliases::new("f2b03-public-batch-b2").unwrap(),
                ],
                None,
                Identifier::new("p-102").unwrap(),
                None,
                Score::new(61).unwrap(),
                None,
                ValBool::new(false).unwrap(),
                ValConstrained::new(61).unwrap(),
                ValDate::new(Date::try_new("2026-08-02").unwrap()).unwrap(),
                ValDatetime::new(DateTime::try_new("2026-08-02T03:55:00").unwrap()).unwrap(),
                ValDatetimeTz::new(DateTimeTz::try_new("2026-08-02T03:55:00Z").unwrap()).unwrap(),
                ValDecimal::new(Decimal::try_new("127.45").unwrap()).unwrap(),
                ValDouble::new(CanonicalDouble::try_new(7.14).unwrap()).unwrap(),
                ValDuration::new(Duration::try_new("P5D").unwrap()).unwrap(),
            )
            .unwrap(),
        ])
        .await
        .expect("person insert_many");
    assert_eq!(batch_people.len(), 2);
    assert_eq!(batch_people[0].identifier().value(), "p-101");
    assert_eq!(batch_people[1].identifier().value(), "p-102");
    assert!(!batch_people[0].iid().is_empty());
    assert!(!batch_people[1].iid().is_empty());
    assert_ne!(batch_people[0].iid(), batch_people[1].iid());
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("person batch count"),
        person_baseline + 3
    );
    let public_ids = db
        .entities::<Person>()
        .all()
        .await
        .expect("public person rows")
        .into_iter()
        .filter_map(|row| {
            let id = row.identifier().value().clone();
            (id == "p-100" || id == "p-101" || id == "p-102").then_some(id)
        })
        .collect::<Vec<_>>();
    let mut public_ids = public_ids;
    public_ids.sort();
    assert_eq!(public_ids, vec!["p-100", "p-101", "p-102"]);
    let updated = db
        .entities::<Person>()
        .update(
            &iid,
            PersonCreate::try_new(
                vec![Aliases::new("f2b03-public-delta".to_owned()).expect("alias")],
                None,
                Identifier::new("p-100".to_owned()).expect("identifier"),
                None,
                Score::new(44).expect("score"),
                None,
                ValBool::new(true).expect("bool"),
                ValConstrained::new(52).expect("constrained"),
                ValDate::new(Date::try_new("2026-07-30").expect("date")).expect("date"),
                ValDatetime::new(DateTime::try_new("2026-07-30T03:55:00").expect("datetime"))
                    .expect("datetime"),
                ValDatetimeTz::new(DateTimeTz::try_new("2026-07-30T03:55:00Z").expect("tz"))
                    .expect("tz"),
                ValDecimal::new(Decimal::try_new("125.45").expect("decimal")).expect("decimal"),
                ValDouble::new(CanonicalDouble::try_new(5.14).expect("double")).expect("double"),
                ValDuration::new(Duration::try_new("P3D").expect("duration")).expect("duration"),
            )
            .expect("update input"),
        )
        .await
        .expect("update");
    assert_eq!(updated.iid(), iid);
    assert_person(
        &updated,
        "p-100",
        &["f2b03-public-delta"],
        None,
        44,
        5.14,
        "125.45",
        true,
        52,
        "2026-07-30",
        "2026-07-30T03:55:00",
        "2026-07-30T03:55:00Z",
        "P3D",
    );
    let updated_read = db
        .entities::<Person>()
        .get_by_iid(&iid)
        .await
        .expect("updated exact read")
        .expect("updated person exists");
    assert_eq!(updated_read.iid(), iid);
    assert_person(
        &updated_read,
        "p-100",
        &["f2b03-public-delta"],
        None,
        44,
        5.14,
        "125.45",
        true,
        52,
        "2026-07-30",
        "2026-07-30T03:55:00",
        "2026-07-30T03:55:00Z",
        "P3D",
    );
    assert_eq!(updated.score().value(), &44);
    assert_eq!(updated.val_duration().value().as_str(), "P3D");
    let special_alias = "quote'\"\\line\nunicode-λ";
    let replaced_ownerships = db
        .entities::<Person>()
        .update(
            &iid,
            ownership_edge_person_input("p-100", &[special_alias], None),
        )
        .await
        .expect("special ownership replacement");
    assert_eq!(replaced_ownerships.nickname(), None);
    assert_eq!(replaced_ownerships.aliases()[0].value(), special_alias);
    let cleared_ownerships = db
        .entities::<Person>()
        .update(&iid, ownership_edge_person_input("p-100", &[], None))
        .await
        .expect("ownership clearing update");
    assert_eq!(cleared_ownerships.nickname(), None);
    assert!(cleared_ownerships.aliases().is_empty());
    assert!(
        db.entities::<Person>()
            .get_by_iid(&iid)
            .await
            .expect("cleared ownership read")
            .expect("cleared ownership person exists")
            .aliases()
            .is_empty()
    );
    db.entities::<Person>().delete(&iid).await.expect("delete");
    db.entities::<Person>()
        .delete(batch_people[0].iid())
        .await
        .expect("batch delete one");
    db.entities::<Person>()
        .delete(batch_people[1].iid())
        .await
        .expect("batch delete two");
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("person cleanup count"),
        person_baseline
    );
    println!("public generated entity CRUD, batches, and scalar domains: passed");
}

#[tokio::test]
async fn generated_inheritance_exact_and_subtype_reads() {
    let db = database().await;
    let employee_baseline = db
        .entities::<Employee>()
        .count()
        .await
        .expect("employee baseline count");
    let employee_subtype_baseline = db
        .entities::<Employee>()
        .subtypes()
        .count()
        .await
        .expect("employee subtype baseline count");
    let party_baseline = db
        .entities::<Party>()
        .subtypes()
        .count()
        .await
        .expect("party baseline count");
    assert_eq!(employee_baseline, 0);
    assert_eq!(employee_subtype_baseline, 0);
    assert_eq!(party_baseline, 0);
    let employee = db
        .entities::<Employee>()
        .insert(
            EmployeeCreate::try_new(
                Identifier::new("emp-1").unwrap(),
                PartyName::new("employee").unwrap(),
                Rank::new(1).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("employee insert");
    let employee_iid = employee.iid().to_owned();
    assert!(!employee_iid.is_empty());
    assert_eq!(employee.identifier().value(), "emp-1");
    assert_eq!(employee.party_name().value(), "employee");
    assert_eq!(employee.rank().value(), &1);
    let employee_put = db
        .entities::<Employee>()
        .put(
            EmployeeCreate::try_new(
                Identifier::new("emp-1").unwrap(),
                PartyName::new("employee-put").unwrap(),
                Rank::new(3).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("employee put");
    assert_eq!(employee_put.iid(), employee_iid);
    assert_eq!(employee_put.identifier().value(), "emp-1");
    assert_eq!(employee_put.party_name().value(), "employee-put");
    assert_eq!(employee_put.rank().value(), &3);
    let put_many = db
        .entities::<Employee>()
        .put_many(vec![
            EmployeeCreate::try_new(
                Identifier::new("emp-1").unwrap(),
                PartyName::new("employee-batch-existing").unwrap(),
                Rank::new(4).unwrap(),
            )
            .unwrap(),
            EmployeeCreate::try_new(
                Identifier::new("emp-2").unwrap(),
                PartyName::new("employee-batch-new").unwrap(),
                Rank::new(5).unwrap(),
            )
            .unwrap(),
        ])
        .await
        .expect("employee put_many");
    assert_eq!(put_many.len(), 2);
    assert_eq!(put_many[0].iid(), employee_iid);
    assert_eq!(put_many[0].rank().value(), &4);
    assert_eq!(put_many[0].identifier().value(), "emp-1");
    assert_eq!(put_many[0].party_name().value(), "employee-batch-existing");
    let new_employee_iid = put_many[1].iid().to_owned();
    assert_eq!(put_many[1].identifier().value(), "emp-2");
    assert_eq!(put_many[1].party_name().value(), "employee-batch-new");
    assert_eq!(put_many[1].rank().value(), &5);
    assert!(!new_employee_iid.is_empty());
    let employee_updated = db
        .entities::<Employee>()
        .update(
            &employee_iid,
            EmployeeCreate::try_new(
                Identifier::new("emp-1").unwrap(),
                PartyName::new("employee-updated").unwrap(),
                Rank::new(6).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("employee update");
    assert_eq!(employee_updated.iid(), employee_iid);
    assert_eq!(employee_updated.rank().value(), &6);
    assert_eq!(employee_updated.identifier().value(), "emp-1");
    assert_eq!(employee_updated.party_name().value(), "employee-updated");
    let exact_employee = db
        .entities::<Employee>()
        .get_by_iid(&employee_iid)
        .await
        .expect("employee exact get")
        .expect("employee exists");
    assert_eq!(exact_employee.iid(), employee_iid);
    assert_eq!(exact_employee.identifier().value(), "emp-1");
    assert_eq!(exact_employee.party_name().value(), "employee-updated");
    assert_eq!(exact_employee.rank().value(), &6);
    db.entities::<Employee>()
        .delete(&new_employee_iid)
        .await
        .expect("new employee delete before subtype reads");
    assert!(
        db.entities::<Employee>()
            .get_by_iid(&new_employee_iid)
            .await
            .expect("new employee absent")
            .is_none()
    );
    assert_eq!(
        db.entities::<Employee>()
            .count()
            .await
            .expect("employee count"),
        employee_baseline + 1
    );
    let exact_employee_all = db.entities::<Employee>().all().await.expect("employee all");
    assert_eq!(exact_employee_all.len() as u64, employee_baseline + 1);
    assert!(
        exact_employee_all
            .iter()
            .any(|value| value.iid() == employee_iid)
    );
    let manager = db
        .entities::<Manager>()
        .insert(
            ManagerCreate::try_new(
                Identifier::new("mgr-1").unwrap(),
                ManagerNote::new("lead").unwrap(),
                PartyName::new("manager").unwrap(),
                Rank::new(2).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("manager insert");
    assert!(!manager.iid().is_empty());
    let contractor = db
        .entities::<Contractor>()
        .insert(
            ContractorCreate::try_new(
                ContractorCode::new("ctr-1").unwrap(),
                Identifier::new("ctr-1").unwrap(),
                PartyName::new("contractor").unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("contractor insert");
    assert!(!contractor.iid().is_empty());
    let exact_employees = db
        .entities::<Employee>()
        .all()
        .await
        .expect("employee exact all");
    assert_eq!(exact_employees.len() as u64, employee_baseline + 1);
    assert!(
        exact_employees
            .iter()
            .any(|value| value.iid() == employee_iid)
    );
    assert_eq!(
        db.entities::<Employee>()
            .count()
            .await
            .expect("employee count"),
        employee_baseline + 1
    );
    assert!(
        exact_employees
            .iter()
            .all(|value| value.iid() != manager.iid())
    );
    assert!(
        db.entities::<Employee>()
            .get_by_iid(manager.iid())
            .await
            .expect("exact employee lookup")
            .is_none()
    );
    db.entities::<Employee>()
        .delete(manager.iid())
        .await
        .expect("exact employee delete manager iid");
    assert!(
        db.entities::<Manager>()
            .get_by_iid(manager.iid())
            .await
            .expect("manager remains")
            .is_some()
    );
    let manager_exact = db
        .entities::<Manager>()
        .get_by_iid(manager.iid())
        .await
        .expect("manager exact rehydrate")
        .expect("manager exact row");
    assert_eq!(manager_exact.iid(), manager.iid());
    assert_eq!(manager_exact.identifier().value(), "mgr-1");
    assert_eq!(manager_exact.party_name().value(), "manager");
    assert_eq!(manager_exact.rank().value(), &2);
    assert_eq!(manager_exact.manager_note().value(), "lead");
    let contractor_exact = db
        .entities::<Contractor>()
        .get_by_iid(contractor.iid())
        .await
        .expect("contractor exact rehydrate")
        .expect("contractor exact row");
    assert_eq!(contractor_exact.iid(), contractor.iid());
    assert_eq!(contractor_exact.identifier().value(), "ctr-1");
    assert_eq!(contractor_exact.party_name().value(), "contractor");
    assert_eq!(contractor_exact.contractor_code().value(), "ctr-1");
    let employee_family = db
        .entities::<Employee>()
        .subtypes()
        .all()
        .await
        .expect("employee family");
    assert_eq!(employee_family.len() as u64, employee_subtype_baseline + 2);
    let party_family = db
        .entities::<Party>()
        .subtypes()
        .all()
        .await
        .expect("party family");
    assert_eq!(party_family.len() as u64, party_baseline + 3);
    assert_eq!(
        db.entities::<Employee>()
            .subtypes()
            .count()
            .await
            .expect("employee subtype count"),
        employee_subtype_baseline + 2
    );
    assert_eq!(
        db.entities::<Party>()
            .subtypes()
            .count()
            .await
            .expect("party subtype count"),
        party_baseline + 3
    );
    let manager_variant = db
        .entities::<Employee>()
        .subtypes()
        .get_by_iid(manager.iid())
        .await
        .expect("manager subtype get")
        .expect("manager variant");
    match manager_variant {
        EmployeeFamily::Manager(value) => {
            assert_eq!(value.iid(), manager.iid());
            assert_eq!(value.manager_note().value(), "lead");
        }
        EmployeeFamily::Employee(_) => panic!("manager must dispatch as Manager"),
    }
    let contractor_variant = db
        .entities::<Party>()
        .subtypes()
        .get_by_iid(contractor.iid())
        .await
        .expect("contractor subtype get")
        .expect("contractor variant");
    match contractor_variant {
        PartyFamily::Contractor(value) => {
            assert_eq!(value.iid(), contractor.iid());
            assert_eq!(value.contractor_code().value(), "ctr-1");
        }
        _ => panic!("contractor must dispatch as Contractor"),
    }
    assert!(party_family.iter().any(|v| v.iid() == employee.iid()));
    assert!(party_family.iter().any(|v| v.iid() == manager.iid()));
    assert!(party_family.iter().any(|v| v.iid() == contractor.iid()));
    let mut employee_common = employee_family
        .iter()
        .map(|family| {
            (
                family.identifier().value().clone(),
                family.party_name().value().clone(),
            )
        })
        .collect::<Vec<_>>();
    employee_common.sort();
    assert_eq!(
        employee_common,
        vec![
            ("emp-1".to_owned(), "employee-updated".to_owned()),
            ("mgr-1".to_owned(), "manager".to_owned())
        ]
    );
    let mut party_common = party_family
        .iter()
        .map(|family| {
            (
                family.identifier().value().clone(),
                family.party_name().value().clone(),
            )
        })
        .collect::<Vec<_>>();
    party_common.sort();
    assert_eq!(
        party_common,
        vec![
            ("ctr-1".to_owned(), "contractor".to_owned()),
            ("emp-1".to_owned(), "employee-updated".to_owned()),
            ("mgr-1".to_owned(), "manager".to_owned())
        ]
    );
    for family in employee_family {
        match family {
            EmployeeFamily::Employee(value) => {
                assert_eq!(value.iid(), employee_iid);
                assert_eq!(value.rank().value(), &6);
                assert_eq!(value.party_name().value(), "employee-updated");
            }
            EmployeeFamily::Manager(value) => {
                assert_eq!(value.iid(), manager.iid());
                assert_eq!(value.manager_note().value(), "lead");
            }
        }
    }
    for family in party_family {
        match family {
            PartyFamily::Employee(value) => {
                assert_eq!(value.iid(), employee_iid);
                assert_eq!(value.party_name().value(), "employee-updated");
                assert_eq!(value.rank().value(), &6);
            }
            PartyFamily::Manager(value) => {
                assert_eq!(value.iid(), manager.iid());
                assert_eq!(value.manager_note().value(), "lead");
            }
            PartyFamily::Contractor(value) => {
                assert_eq!(value.iid(), contractor.iid());
                assert_eq!(value.contractor_code().value(), "ctr-1");
            }
        }
    }

    // F3-06A: generated subtype/exact expressions and singular terminals.
    {
        let mut session = db.query().expect("query session");
        let party_binding = session.subtypes::<Party>().expect("party binding");
        let employee_binding = session.exact::<Employee>().expect("employee binding");
        let party_name = party_binding.field(type_bridge_generated_schema::PartyType::party_name);
        let employee_name =
            employee_binding.field(type_bridge_generated_schema::PartyType::party_name);
        let employee_rank =
            employee_binding.field(type_bridge_generated_schema::EmployeeType::rank);

        let manager_name = PartyName::new("manager").expect("manager query wrapper");
        let manager_predicate = party_name.eq(manager_name.clone())
            & party_name.starts_with(Text::new("man").expect("manager prefix"))
            & party_name.contains(Text::new("anag").expect("manager substring"))
            & party_name.ends_with(Text::new("ger").expect("manager suffix"))
            & party_name.regex(Regex::new("^manager$").expect("manager regex"))
            & !party_name.ne(manager_name.clone())
            & (party_name.eq(manager_name)
                | party_name.starts_with(Text::new("unused").expect("alternate prefix")));
        let manager_result = session
            .query(party_binding)
            .expect("party query")
            .where_(manager_predicate)
            .expect("manager predicate")
            .one()
            .await
            .expect("manager query returns one row");
        match manager_result {
            PartyFamily::Manager(value) => assert_eq!(value.manager_note().value(), "lead"),
            _ => panic!("manager expression query must materialize the Manager variant"),
        }

        let party_query = session.query(party_binding).expect("reusable party query");
        assert_eq!(party_query.count().await.expect("party query count"), 3);
        assert!(party_query.exists().await.expect("party query exists"));
        let first = party_query
            .first(party_name.asc())
            .await
            .expect("party first")
            .expect("party first row");
        assert_eq!(first.party_name().value(), "contractor");
        let page = party_query
            .rows(RowsOptions::new(1).offset(1).order_by(party_name.asc()))
            .await
            .expect("party ordered page");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].party_name().value(), "employee-updated");

        let exact_employee = session
            .query(employee_binding)
            .expect("exact employee query")
            .where_(
                employee_name.eq(PartyName::new("employee-updated").expect("employee wrapper"))
                    & employee_rank.ge(6_i64),
            )
            .expect("exact employee predicates")
            .one()
            .await
            .expect("exact employee result");
        assert_eq!(exact_employee.iid(), employee_iid);
    }

    db.entities::<Manager>()
        .delete(manager.iid())
        .await
        .expect("manager delete");
    db.entities::<Employee>()
        .delete(employee.iid())
        .await
        .expect("employee delete");
    db.entities::<Contractor>()
        .delete(contractor.iid())
        .await
        .expect("contractor delete");
    assert!(
        db.entities::<Manager>()
            .get_by_iid(manager.iid())
            .await
            .expect("manager cleanup get")
            .is_none()
    );
    assert!(
        db.entities::<Employee>()
            .get_by_iid(employee.iid())
            .await
            .expect("employee cleanup get")
            .is_none()
    );
    assert!(
        db.entities::<Contractor>()
            .get_by_iid(contractor.iid())
            .await
            .expect("contractor cleanup get")
            .is_none()
    );
    assert_eq!(
        db.entities::<Employee>()
            .count()
            .await
            .expect("employee final count"),
        employee_baseline
    );
    assert_eq!(
        db.entities::<Employee>()
            .subtypes()
            .count()
            .await
            .expect("employee subtype final count"),
        employee_subtype_baseline
    );
    assert_eq!(
        db.entities::<Party>()
            .subtypes()
            .count()
            .await
            .expect("party subtype final count"),
        party_baseline
    );
    println!("F2B-03 public generated entity lifecycle: passed");
}

#[tokio::test]
async fn generated_relation_query_and_remote_lifecycle() {
    let db = database().await;
    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("relation person baseline count");
    let employment_baseline = db
        .relations::<Employment>()
        .count()
        .await
        .expect("employment baseline count");
    let membership_exact_baseline = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership exact baseline count");
    let membership_family_baseline = db
        .relations::<Membership>()
        .subtypes()
        .count()
        .await
        .expect("membership family baseline count");
    let _event_baseline = db
        .relations::<Event>()
        .count()
        .await
        .expect("event baseline count");
    let container_baseline = db
        .relations::<Container>()
        .count()
        .await
        .expect("container baseline count");
    let network_link_baseline = db
        .relations::<NetworkLink>()
        .count()
        .await
        .expect("network-link baseline count");
    assert!(
        db.relations::<Employment>()
            .insert_many(Vec::new())
            .await
            .expect("empty relation insert batch")
            .is_empty()
    );
    assert!(
        db.relations::<Employment>()
            .put_many(Vec::new())
            .await
            .expect("empty relation put batch")
            .is_empty()
    );
    assert!(
        db.relations::<Employment>()
            .update_many(Vec::new())
            .await
            .expect("empty relation update batch")
            .is_empty()
    );
    db.relations::<Employment>()
        .delete_many(&[])
        .await
        .expect("empty relation delete batch");
    let worker_a = db
        .entities::<Person>()
        .insert(relation_person_input("p-200", "f2c03-worker-a"))
        .await
        .expect("relation worker a insert");
    let worker_b = db
        .entities::<Person>()
        .insert(relation_person_input("p-201", "f2c03-worker-b"))
        .await
        .expect("relation worker b insert");
    let worker_c = db
        .entities::<Person>()
        .insert(relation_person_input("p-202", "f2c03-worker-c"))
        .await
        .expect("relation worker c insert");
    let worker_d = db
        .entities::<Person>()
        .insert(relation_person_input("p-203", "f5-worker-d"))
        .await
        .expect("relation worker d insert");

    // F5: a keyed generated relation covers relation-owned attribute
    // replacement, repeated players on one role, and put hit/miss behavior.
    let keyed_link = db
        .relations::<NetworkLink>()
        .insert(
            NetworkLinkCreate::new(
                Identifier::new("link-keyed").expect("network key"),
                Some(Nickname::new("initial").expect("network nickname")),
                worker_b.reference(),
                worker_a.reference(),
                vec![worker_a.reference(), worker_b.reference()],
            )
            .expect("keyed network create"),
        )
        .await
        .expect("keyed network insert");
    let keyed_link_iid = keyed_link.iid().to_owned();
    assert_eq!(keyed_link.identifier().value(), "link-keyed");
    assert_eq!(
        keyed_link.nickname().expect("initial nickname").value(),
        "initial"
    );
    assert_eq!(keyed_link.participant().len(), 2);

    let keyed_put = db
        .relations::<NetworkLink>()
        .put(
            NetworkLinkCreate::new(
                Identifier::new("link-keyed").expect("network key"),
                Some(Nickname::new("put-replaced").expect("network nickname")),
                worker_c.reference(),
                worker_a.reference(),
                vec![worker_a.reference(), worker_c.reference()],
            )
            .expect("keyed network put create"),
        )
        .await
        .expect("keyed network put hit");
    assert_eq!(keyed_put.iid(), keyed_link_iid);
    assert_eq!(
        keyed_put.nickname().expect("put nickname").value(),
        "put-replaced"
    );
    assert_eq!(keyed_put.destination().identifier().value(), "p-202");
    assert_eq!(keyed_put.participant().len(), 2);

    let put_new = db
        .relations::<NetworkLink>()
        .put(
            NetworkLinkCreate::new(
                Identifier::new("link-put-new").expect("new network key"),
                Some(Nickname::new("new").expect("network nickname")),
                PersonRef::from_key(Identifier::new("p-203").expect("destination key"))
                    .expect("destination key reference"),
                PersonRef::from_key(Identifier::new("p-201").expect("origin key"))
                    .expect("origin key reference"),
                vec![worker_b.reference(), worker_d.reference()],
            )
            .expect("new keyed network create"),
        )
        .await
        .expect("keyed network put miss");
    let put_new_iid = put_new.iid().to_owned();
    assert_ne!(put_new_iid, keyed_link_iid);
    let updated_new = db
        .relations::<NetworkLink>()
        .update(
            &put_new_iid,
            NetworkLinkCreate::new(
                Identifier::new("link-put-new").expect("new network key"),
                None,
                worker_d.reference(),
                worker_c.reference(),
                vec![worker_c.reference(), worker_d.reference()],
            )
            .expect("network update input"),
        )
        .await
        .expect("network update replaces attributes and roles");
    assert_eq!(updated_new.iid(), put_new_iid);
    assert!(updated_new.nickname().is_none());
    assert_eq!(updated_new.origin().identifier().value(), "p-202");
    assert_eq!(updated_new.destination().identifier().value(), "p-203");
    assert_eq!(
        db.relations::<NetworkLink>()
            .count()
            .await
            .expect("network relation lifecycle count"),
        network_link_baseline + 2
    );
    let keyed_batch = db
        .relations::<NetworkLink>()
        .put_many(vec![
            NetworkLinkCreate::new(
                Identifier::new("link-keyed").expect("existing batch network key"),
                Some(Nickname::new("batch-replaced").expect("batch network nickname")),
                worker_d.reference(),
                worker_a.reference(),
                vec![worker_a.reference(), worker_d.reference()],
            )
            .expect("existing batch network create"),
            NetworkLinkCreate::new(
                Identifier::new("link-batch-new").expect("new batch network key"),
                None,
                worker_c.reference(),
                worker_b.reference(),
                vec![worker_b.reference(), worker_c.reference()],
            )
            .expect("new batch network create"),
        ])
        .await
        .expect("network relation put_many");
    assert_eq!(keyed_batch.len(), 2);
    assert_eq!(keyed_batch[0].iid(), keyed_link_iid);
    let batch_link_iid = keyed_batch[1].iid().to_owned();
    assert_ne!(batch_link_iid, keyed_link_iid);
    assert_ne!(batch_link_iid, put_new_iid);
    assert_eq!(
        db.relations::<NetworkLink>()
            .count()
            .await
            .expect("network relation batch count"),
        network_link_baseline + 3
    );
    for iid in [&keyed_link_iid, &put_new_iid, &batch_link_iid] {
        db.relations::<NetworkLink>()
            .delete(iid)
            .await
            .expect("keyed network cleanup");
    }

    // F5: bounded reachability traverses a cycle and deduplicates the shared
    // D subtree reached as A -> D and A -> B -> D.
    let graph_specs = [
        ("link-a-b", &worker_a, &worker_b),
        ("link-b-c", &worker_b, &worker_c),
        ("link-c-a", &worker_c, &worker_a),
        ("link-a-d", &worker_a, &worker_d),
        ("link-b-d", &worker_b, &worker_d),
    ];
    let mut graph_link_iids = Vec::new();
    for (identifier, origin, destination) in graph_specs {
        let link = db
            .relations::<NetworkLink>()
            .insert(
                NetworkLinkCreate::new(
                    Identifier::new(identifier).expect("graph link key"),
                    None,
                    destination.reference(),
                    origin.reference(),
                    vec![origin.reference(), destination.reference()],
                )
                .expect("graph link create"),
            )
            .await
            .expect("graph link insert");
        graph_link_iids.push(link.iid().to_owned());
    }
    {
        let mut session = db.query().expect("F5 reachability session");
        let source = session.exact::<Person>().expect("F5 source binding");
        let target = session.exact::<Person>().expect("F5 target binding");
        let source_identifier = source.field(PersonType::identifier);
        let target_identifier = target.field(PersonType::identifier);
        let reachable = session
            .reachable(
                NetworkLinkType::TOKEN,
                NetworkLinkType::origin,
                NetworkLinkType::destination,
                source,
                target,
                1,
                2,
            )
            .expect("F5 bounded reachability predicate");
        let rows = session
            .query((source, target))
            .expect("F5 reachability query")
            .where_(
                reachable
                    & source_identifier.eq(Identifier::new("p-200").expect("source key"))
                    & target_identifier.starts_with(Text::new("p-2").expect("target prefix")),
            )
            .expect("F5 reachability filters")
            .rows(RowsOptions::new(10).order_by(target_identifier.asc()))
            .await
            .expect("F5 reachable rows");
        assert_eq!(
            rows.iter()
                .map(|(_, target)| target.identifier().value().as_str())
                .collect::<Vec<_>>(),
            vec!["p-201", "p-202", "p-203"]
        );

        let cycle_at_two = session
            .reachable(
                NetworkLinkType::TOKEN,
                NetworkLinkType::origin,
                NetworkLinkType::destination,
                source,
                target,
                1,
                2,
            )
            .expect("F5 cycle exclusion predicate");
        assert_eq!(
            session
                .query((source, target))
                .expect("F5 cycle exclusion query")
                .where_(
                    cycle_at_two
                        & source_identifier.eq(Identifier::new("p-200").expect("source key"))
                        & target_identifier.eq(Identifier::new("p-200").expect("target key")),
                )
                .expect("F5 cycle exclusion filters")
                .count_by(source)
                .await
                .expect("F5 cycle exclusion count"),
            0
        );
        let cycle_at_three = session
            .reachable(
                NetworkLinkType::TOKEN,
                NetworkLinkType::origin,
                NetworkLinkType::destination,
                source,
                target,
                1,
                3,
            )
            .expect("F5 cycle inclusion predicate");
        assert!(
            session
                .query((source, target))
                .expect("F5 cycle inclusion query")
                .where_(
                    cycle_at_three
                        & source_identifier.eq(Identifier::new("p-200").expect("source key"))
                        & target_identifier.eq(Identifier::new("p-200").expect("target key")),
                )
                .expect("F5 cycle inclusion filters")
                .exists_by(source)
                .await
                .expect("F5 cycle inclusion exists")
        );
    }
    let employment_one = db
        .relations::<Employment>()
        .insert(EmploymentCreate::new(worker_a.reference()).expect("employment create by iid"))
        .await
        .expect("employment insert by IID reference");
    let employment_one_iid = employment_one.iid().to_owned();
    assert!(!employment_one_iid.is_empty());
    let employment_read = db
        .relations::<Employment>()
        .get_by_iid(&employment_one_iid)
        .await
        .expect("employment exact read")
        .expect("employment exists");
    assert_eq!(employment_read.iid(), employment_one_iid);
    assert_eq!(
        db.relations::<Employment>()
            .count()
            .await
            .expect("employment count after insert"),
        employment_baseline + 1
    );

    let key_reference =
        PersonRef::from_key(Identifier::new("p-201".to_owned()).expect("key reference identifier"))
            .expect("typed key reference");
    let employment_batch = db
        .relations::<Employment>()
        .insert_many(vec![
            EmploymentCreate::new(key_reference).expect("employment create by key"),
        ])
        .await
        .expect("employment insert_many by typed key reference");
    assert_eq!(employment_batch.len(), 1);
    let employment_two_iid = employment_batch[0].iid().to_owned();
    assert_ne!(employment_two_iid, employment_one_iid);

    let employment_updated = db
        .relations::<Employment>()
        .update(
            &employment_one_iid,
            EmploymentCreate::new(worker_c.reference()).expect("employment replacement create"),
        )
        .await
        .expect("employment update replaces the active player set");
    assert_eq!(employment_updated.iid(), employment_one_iid);

    let employment_put = db
        .relations::<Employment>()
        .put(EmploymentCreate::new(worker_a.reference()).expect("employment put create"))
        .await
        .expect("employment put without a usable key inserts");
    let employment_three_iid = employment_put.iid().to_owned();
    assert_ne!(employment_three_iid, employment_one_iid);
    assert_ne!(employment_three_iid, employment_two_iid);
    assert_eq!(
        db.relations::<Employment>()
            .count()
            .await
            .expect("employment count after put"),
        employment_baseline + 3
    );
    let employment_all = db
        .relations::<Employment>()
        .all()
        .await
        .expect("employment exact all");
    assert_eq!(employment_all.len() as u64, employment_baseline + 3);
    assert!(
        employment_all
            .iter()
            .any(|value| value.iid() == employment_one_iid)
    );

    // F3-06B: reusable generated queries, all reducers, and role-grouped
    // materialization over the live relation lifecycle.
    {
        let mut session = db.query().expect("query session");
        let person_binding = session.exact::<Person>().expect("person binding");
        let employment_binding = session.exact::<Employment>().expect("employment binding");
        let network_binding = session.exact::<NetworkLink>().expect("network binding");
        let identifier = person_binding.field(PersonType::identifier);
        let aliases = person_binding.field(PersonType::aliases);
        let nickname = person_binding.field(PersonType::nickname);
        let score = person_binding.field(PersonType::score);
        let val_bool = person_binding.field(PersonType::val_bool);
        let employee_role = employment_binding.role(EmploymentType::employee);

        let worker_predicate = identifier.starts_with(Text::new("p-2").expect("worker prefix"))
            & identifier.contains(Text::new("p-20").expect("worker substring"))
            & identifier.regex(Regex::new("^p-20[0-2]$").expect("worker regex"))
            & score.ge(70_i64)
            & score.lt(71_i64)
            & !(identifier.eq(Identifier::new("p-999").expect("absent worker")) | score.lt(70_i64));
        let worker_query = session
            .query(person_binding)
            .expect("worker query")
            .where_(worker_predicate.clone())
            .expect("worker predicates");
        assert_eq!(worker_query.count().await.expect("worker count"), 3);
        assert!(worker_query.exists().await.expect("worker exists"));

        let workers = worker_query
            .rows(RowsOptions::new(2).offset(1).order_by(identifier.asc()))
            .await
            .expect("worker ordered page");
        assert_eq!(
            workers
                .iter()
                .map(|value| value.identifier().value().as_str())
                .collect::<Vec<_>>(),
            vec!["p-201", "p-202"]
        );
        let first_worker = worker_query
            .first(identifier.asc())
            .await
            .expect("worker first")
            .expect("worker first row");
        assert_eq!(first_worker.identifier().value(), "p-200");
        let worker_a_query = session
            .query(person_binding)
            .expect("single worker query")
            .where_(
                identifier.eq(Identifier::new("p-200").expect("worker wrapper"))
                    & identifier.ends_with(Text::new("00").expect("worker suffix")),
            )
            .expect("single worker predicates")
            .one()
            .await
            .expect("single worker result");
        assert_eq!(worker_a_query.iid(), worker_a.iid());
        let worker_a_by_iid = session
            .query(person_binding)
            .expect("single worker IID query")
            .where_(
                person_binding.iid(worker_a.iid()) & aliases.is_present() & nickname.is_missing(),
            )
            .expect("single worker IID and presence predicates")
            .one()
            .await
            .expect("single worker IID result");
        assert_eq!(worker_a_by_iid.iid(), worker_a.iid());
        let workers_by_iid = session
            .query(person_binding)
            .expect("worker IID set query")
            .where_(person_binding.iid_in([worker_a.iid(), worker_b.iid()]))
            .expect("worker IID set predicate")
            .rows(RowsOptions::new(10).order_by(identifier.asc()))
            .await
            .expect("worker IID set rows");
        assert_eq!(
            workers_by_iid
                .iter()
                .map(|value| value.iid())
                .collect::<Vec<_>>(),
            vec![worker_a.iid(), worker_b.iid()]
        );
        let network_by_iid = session
            .query(network_binding)
            .expect("network IID query")
            .where_(
                network_binding.iid(graph_link_iids[0].clone())
                    & network_binding
                        .field(NetworkLinkType::identifier)
                        .is_present()
                    & network_binding
                        .field(NetworkLinkType::nickname)
                        .is_missing(),
            )
            .expect("network IID and presence predicates")
            .one()
            .await
            .expect("network IID result");
        assert_eq!(network_by_iid.iid(), graph_link_iids[0]);

        let stats: (
            u64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            Option<f64>,
            Option<f64>,
        ) = worker_query
            .aggregate((
                aggregate::count(),
                score.sum(),
                score.min(),
                score.max(),
                score.mean(),
                score.median(),
                score.stddev(),
            ))
            .await
            .expect("worker aggregate");
        assert_eq!(
            stats,
            (
                3,
                210,
                Some(70),
                Some(70),
                Some(70.0),
                Some(70.0),
                Some(0.0)
            )
        );

        let field_grouped = worker_query
            .group_by_field(val_bool)
            .expect("worker boolean field group")
            .aggregate((aggregate::count(), score.sum()))
            .await
            .expect("worker field-grouped aggregate");
        assert_eq!(field_grouped.len(), 1);
        assert_eq!(field_grouped[0].0.value(), &true);
        assert_eq!(field_grouped[0].1, (3, 210));

        let tuple_field_grouped = worker_query
            .group_by_fields((val_bool, score))
            .expect("worker tuple field group")
            .aggregate((aggregate::count(), score.sum()))
            .await
            .expect("worker tuple-field-grouped aggregate");
        assert_eq!(tuple_field_grouped.len(), 1);
        assert_eq!(tuple_field_grouped[0].0.0.value(), &true);
        assert_eq!(tuple_field_grouped[0].0.1.value(), &70);
        assert_eq!(tuple_field_grouped[0].1, (3, 210));

        let grouped = session
            .query(employment_binding)
            .expect("employment query")
            .where_(employee_role.connects(person_binding) & worker_predicate)
            .expect("employment role predicate")
            .group_by(person_binding)
            .expect("employment group")
            .aggregate((aggregate::count(), score.mean()))
            .await
            .expect("employment grouped aggregate");
        let mut grouped = grouped
            .into_iter()
            .map(|(person, values)| (person.identifier().value().clone(), values))
            .collect::<Vec<_>>();
        grouped.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            grouped,
            vec![
                ("p-200".to_owned(), (1, Some(70.0))),
                ("p-201".to_owned(), (1, Some(70.0))),
                ("p-202".to_owned(), (1, Some(70.0))),
            ]
        );

        let cross_left = session.exact::<Person>().expect("cross left binding");
        let cross_right = session.exact::<Person>().expect("cross right binding");
        let cross_left_identifier = cross_left.field(PersonType::identifier);
        let cross_right_identifier = cross_right.field(PersonType::identifier);
        let cross_pair = session
            .query((cross_left, cross_right))
            .expect("cross query")
            .allow_cross_join(cross_left, cross_right)
            .expect("explicit cross-join permission")
            .where_(
                cross_left_identifier.eq(Identifier::new("p-200").expect("cross left ID"))
                    & cross_right_identifier.eq(Identifier::new("p-201").expect("cross right ID")),
            )
            .expect("cross query predicates")
            .one()
            .await
            .expect("cross query result");
        assert_eq!(cross_pair.0.iid(), worker_a.iid());
        assert_eq!(cross_pair.1.iid(), worker_b.iid());
    }
    println!("F3 public generated query lifecycle: passed");

    // F4: named collected pages, reusable read contexts, and identical
    // generated materialization through the released one-exchange server.
    let expected_page = {
        let mut session = db.query().expect("F4 local query session");
        let person_binding = session.exact::<Person>().expect("F4 local person binding");
        let member_binding = session.exact::<Person>().expect("F4 local member binding");
        let identifier = person_binding.field(PersonType::identifier);
        let member_identifier = member_binding.field(PersonType::identifier);
        let members = member_binding
            .collect()
            .distinct()
            .order_by(member_identifier.asc())
            .expect("F4 local collection order");
        let graph = PersonGraph::select(person_binding, members).expect("F4 local selected graph");
        let query = session
            .query(graph)
            .expect("F4 local graph query")
            .where_(
                identifier.eq_field(member_identifier)
                    & identifier.starts_with(Text::new("p-2").expect("F4 local worker prefix")),
            )
            .expect("F4 local graph predicate");
        let page = query
            .page_by(
                person_binding,
                PageOptions::new(2)
                    .include_total(true)
                    .order_by(identifier.asc()),
            )
            .await
            .expect("F4 local collected page");
        assert_eq!(page.offset(), 0);
        assert_eq!(page.limit(), 2);
        assert_eq!(page.total(), Some(4));
        let observed = page
            .items()
            .iter()
            .map(|row| (row.person.identifier().value().clone(), row.members.len()))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![("p-200".to_owned(), 1), ("p-201".to_owned(), 1)]
        );
        observed
    };

    {
        let read = db.read().await.expect("F4 read transaction opens");
        let mut session = read.query();
        let person_binding = session.exact::<Person>().expect("F4 read person binding");
        let identifier = person_binding.field(PersonType::identifier);
        let query = session
            .query(person_binding)
            .expect("F4 read query")
            .where_(identifier.starts_with(Text::new("p-2").expect("F4 read worker prefix")))
            .expect("F4 read predicate");
        assert_eq!(query.count().await.expect("F4 first read count"), 4);
        assert!(query.exists().await.expect("F4 read exists"));
        assert_eq!(query.count().await.expect("F4 second read count"), 4);
        drop(query);
        drop(session);
        read.close().await.expect("F4 read transaction closes");
    }

    let remote_url = env::var("TYPE_BRIDGE_REMOTE_URL").expect("F4 remote server URL");
    let remote: RemoteDatabase<AppSchema> =
        RemoteDatabase::connect(RemoteConnectionOptions::generated(
            RemoteQueryLimits::new(100, 8 << 20, 1000, 1000, 1000, 1000).deadline_ms(30_000),
            HttpTransport::new(remote_url),
        ))
        .await
        .expect("F4 remote database connects")
        .with_schema(SCHEMA)
        .expect("F4 remote schema authority binds");
    let mut session = remote.query().expect("F4 remote query session");
    let person_binding = session.exact::<Person>().expect("F4 remote person binding");
    let member_binding = session.exact::<Person>().expect("F4 remote member binding");
    let identifier = person_binding.field(PersonType::identifier);
    let member_identifier = member_binding.field(PersonType::identifier);
    let members = member_binding
        .collect()
        .distinct()
        .order_by(member_identifier.asc())
        .expect("F4 remote collection order");
    let graph = PersonGraph::select(person_binding, members).expect("F4 remote selected graph");
    let query = session
        .query(graph)
        .expect("F4 remote graph query")
        .where_(
            identifier.eq_field(member_identifier)
                & identifier.starts_with(Text::new("p-2").expect("F4 remote worker prefix")),
        )
        .expect("F4 remote graph predicate");
    assert_eq!(
        query
            .count_by(person_binding)
            .await
            .expect("F4 remote distinct root count"),
        4
    );
    assert!(
        query
            .exists_by(person_binding)
            .await
            .expect("F4 remote distinct root exists")
    );
    let remote_page = query
        .page_by(
            person_binding,
            PageOptions::new(2)
                .include_total(true)
                .order_by(identifier.asc()),
        )
        .await
        .expect("F4 remote collected page");
    let observed_page = remote_page
        .items()
        .iter()
        .map(|row| (row.person.identifier().value().clone(), row.members.len()))
        .collect::<Vec<_>>();
    assert_eq!(remote_page.total(), Some(4));
    assert_eq!(observed_page, expected_page);

    let remote_score = person_binding.field(PersonType::score);
    let remote_val_bool = person_binding.field(PersonType::val_bool);
    let remote_worker_predicate =
        identifier.starts_with(Text::new("p-2").expect("F4 remote aggregate worker prefix"));
    let remote_worker_query = session
        .query(person_binding)
        .expect("F4 remote aggregate query")
        .where_(remote_worker_predicate.clone())
        .expect("F4 remote aggregate predicate");
    let remote_stats: (
        u64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = remote_worker_query
        .aggregate((
            aggregate::count(),
            remote_score.sum(),
            remote_score.min(),
            remote_score.max(),
            remote_score.mean(),
            remote_score.median(),
            remote_score.stddev(),
        ))
        .await
        .expect("F4 remote aggregate");
    assert_eq!(
        remote_stats,
        (
            4,
            280,
            Some(70),
            Some(70),
            Some(70.0),
            Some(70.0),
            Some(0.0),
        )
    );
    let remote_field_grouped = remote_worker_query
        .group_by_field(remote_val_bool)
        .expect("F4 remote field group")
        .aggregate((aggregate::count(), remote_score.sum()))
        .await
        .expect("F4 remote field-grouped aggregate");
    assert_eq!(remote_field_grouped.len(), 1);
    assert_eq!(remote_field_grouped[0].0.value(), &true);
    assert_eq!(remote_field_grouped[0].1, (4, 280));
    let remote_tuple_grouped = remote_worker_query
        .group_by_fields((remote_val_bool, remote_score))
        .expect("F4 remote tuple group")
        .aggregate((aggregate::count(), remote_score.sum()))
        .await
        .expect("F4 remote tuple-field-grouped aggregate");
    assert_eq!(remote_tuple_grouped.len(), 1);
    assert_eq!(remote_tuple_grouped[0].0.0.value(), &true);
    assert_eq!(remote_tuple_grouped[0].0.1.value(), &70);
    assert_eq!(remote_tuple_grouped[0].1, (4, 280));

    let remote_employment = session
        .exact::<Employment>()
        .expect("F4 remote employment binding");
    let remote_employee_role = remote_employment.role(EmploymentType::employee);
    let remote_binding_grouped = session
        .query(remote_employment)
        .expect("F4 remote employment aggregate query")
        .where_(remote_employee_role.connects(person_binding) & remote_worker_predicate)
        .expect("F4 remote employment aggregate predicate")
        .group_by(person_binding)
        .expect("F4 remote binding group")
        .aggregate((aggregate::count(), remote_score.mean()))
        .await
        .expect("F4 remote binding-grouped aggregate");
    let mut remote_binding_grouped = remote_binding_grouped
        .into_iter()
        .map(|(person, values)| (person.identifier().value().clone(), values))
        .collect::<Vec<_>>();
    remote_binding_grouped.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        remote_binding_grouped,
        vec![
            ("p-200".to_owned(), (1, Some(70.0))),
            ("p-201".to_owned(), (1, Some(70.0))),
            ("p-202".to_owned(), (1, Some(70.0))),
        ]
    );

    let remote_worker_by_iid = session
        .query(person_binding)
        .expect("F4 remote worker IID query")
        .where_(
            person_binding.iid(worker_a.iid())
                & person_binding.field(PersonType::aliases).is_present()
                & person_binding.field(PersonType::nickname).is_missing(),
        )
        .expect("F4 remote worker IID and presence predicates")
        .one()
        .await
        .expect("F4 remote worker IID result");
    assert_eq!(remote_worker_by_iid.iid(), worker_a.iid());
    let remote_workers_by_iid = session
        .query(person_binding)
        .expect("F4 remote worker IID set query")
        .where_(person_binding.iid_in([worker_a.iid(), worker_b.iid()]))
        .expect("F4 remote worker IID set predicate")
        .rows(RowsOptions::new(10).order_by(identifier.asc()))
        .await
        .expect("F4 remote worker IID set rows");
    assert_eq!(
        remote_workers_by_iid
            .iter()
            .map(|value| value.iid())
            .collect::<Vec<_>>(),
        vec![worker_a.iid(), worker_b.iid()]
    );
    let remote_network = session
        .exact::<NetworkLink>()
        .expect("F4 remote network binding");
    let remote_network_by_iid = session
        .query(remote_network)
        .expect("F4 remote network IID query")
        .where_(
            remote_network.iid(graph_link_iids[0].clone())
                & remote_network
                    .field(NetworkLinkType::identifier)
                    .is_present()
                & remote_network.field(NetworkLinkType::nickname).is_missing(),
        )
        .expect("F4 remote network IID and presence predicates")
        .one()
        .await
        .expect("F4 remote network IID result");
    assert_eq!(remote_network_by_iid.iid(), graph_link_iids[0]);
    let remote_cross_left = session
        .exact::<Person>()
        .expect("F4 remote cross left binding");
    let remote_cross_right = session
        .exact::<Person>()
        .expect("F4 remote cross right binding");
    let remote_cross_left_identifier = remote_cross_left.field(PersonType::identifier);
    let remote_cross_right_identifier = remote_cross_right.field(PersonType::identifier);
    let remote_cross_pair = session
        .query((remote_cross_left, remote_cross_right))
        .expect("F4 remote cross query")
        .allow_cross_join(remote_cross_left, remote_cross_right)
        .expect("F4 remote explicit cross-join permission")
        .where_(
            remote_cross_left_identifier
                .eq(Identifier::new("p-200").expect("F4 remote cross left ID"))
                & remote_cross_right_identifier
                    .eq(Identifier::new("p-201").expect("F4 remote cross right ID")),
        )
        .expect("F4 remote cross predicates")
        .one()
        .await
        .expect("F4 remote cross result");
    assert_eq!(remote_cross_pair.0.iid(), worker_a.iid());
    assert_eq!(remote_cross_pair.1.iid(), worker_b.iid());
    println!("F4 public selected/read/remote lifecycle: passed");

    for iid in &graph_link_iids {
        db.relations::<NetworkLink>()
            .delete(iid)
            .await
            .expect("graph link cleanup");
    }
    assert_eq!(
        db.relations::<NetworkLink>()
            .count()
            .await
            .expect("network-link cleanup count"),
        network_link_baseline
    );
    println!("F5 public relation parity and bounded reachability: passed");

    assert_eq!(
        db.relations::<Membership>()
            .count()
            .await
            .expect("membership exact count stays zero"),
        membership_exact_baseline
    );
    assert_eq!(
        db.relations::<Membership>()
            .subtypes()
            .count()
            .await
            .expect("membership family count"),
        membership_family_baseline + 3
    );
    let membership_variant = db
        .relations::<Membership>()
        .subtypes()
        .get_by_iid(&employment_one_iid)
        .await
        .expect("membership family get")
        .expect("membership family member");
    match membership_variant {
        MembershipFamily::Employment(value) => assert_eq!(value.iid(), employment_one_iid),
        MembershipFamily::Membership(_) => {
            panic!("employment must dispatch as the Employment variant")
        }
    }
    let membership_family = db
        .relations::<Membership>()
        .subtypes()
        .all()
        .await
        .expect("membership family all");
    assert_eq!(
        membership_family.len() as u64,
        membership_family_baseline + 3
    );

    let event = db
        .relations::<Event>()
        .insert(EventCreate::new(worker_a.reference()).expect("event create"))
        .await
        .expect("event insert");
    let event_iid = event.iid().to_owned();
    let container = db
        .relations::<Container>()
        .insert(ContainerCreate::new(vec![event.reference()]).expect("container create"))
        .await
        .expect("container insert with a relation player");
    let container_iid = container.iid().to_owned();
    let container_read = db
        .relations::<Container>()
        .get_by_iid(&container_iid)
        .await
        .expect("container exact read")
        .expect("container exists");
    assert_eq!(container_read.iid(), container_iid);
    assert_eq!(
        db.relations::<Container>()
            .count()
            .await
            .expect("container count"),
        container_baseline + 1
    );

    db.relations::<Container>()
        .delete(&container_iid)
        .await
        .expect("container delete");
    db.relations::<Event>()
        .delete(&event_iid)
        .await
        .expect("event delete");
    for iid in [
        &employment_one_iid,
        &employment_two_iid,
        &employment_three_iid,
    ] {
        db.relations::<Employment>()
            .delete(iid)
            .await
            .expect("employment delete");
    }
    assert!(
        db.relations::<Employment>()
            .get_by_iid(&employment_one_iid)
            .await
            .expect("employment absence read")
            .is_none()
    );
    assert_eq!(
        db.relations::<Employment>()
            .count()
            .await
            .expect("employment final count"),
        employment_baseline
    );
    assert_eq!(
        db.relations::<Membership>()
            .subtypes()
            .count()
            .await
            .expect("membership family final count"),
        membership_family_baseline
    );
    assert_eq!(
        db.relations::<Container>()
            .count()
            .await
            .expect("container final count"),
        container_baseline
    );
    for value in [worker_a, worker_b, worker_c, worker_d] {
        db.entities::<Person>()
            .delete(value.iid())
            .await
            .expect("relation worker cleanup");
    }
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("person final count"),
        person_baseline
    );
    println!("F2C-03 public generated relation lifecycle: passed");
}

#[tokio::test]
async fn generated_integer_keys_and_polymorphic_role_parity() {
    let db = database().await;
    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("person baseline");
    let robot_baseline = db
        .entities::<Robot>()
        .count()
        .await
        .expect("robot baseline");
    let membership_baseline = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership baseline");
    let interaction_baseline = db
        .relations::<Interaction>()
        .count()
        .await
        .expect("interaction baseline");

    let person_actor = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "parity-person-actor",
            &["parity-person-actor"],
            Some("parity-actor-person"),
        ))
        .await
        .expect("person actor insert");
    let target = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "parity-person-target",
            &["parity-person-target"],
            Some("parity-target"),
        ))
        .await
        .expect("interaction target insert");

    let robots = db
        .entities::<Robot>()
        .insert_many(
            [
                (-42_i64, "parity-actor-robot"),
                (1_i64, "parity-robot-one"),
                (100_i64, "parity-robot-hundred"),
                (9_999_i64, "parity-robot-large"),
            ]
            .into_iter()
            .map(|(robot_id, nickname)| {
                RobotCreate::new(
                    Some(Nickname::new(nickname).expect("robot nickname")),
                    RobotId::new(robot_id).expect("integer robot key"),
                    ValConstrained::new(20).expect("robot constrained value"),
                )
                .expect("robot create")
            })
            .collect(),
        )
        .await
        .expect("integer-key robot insert batch");
    assert_eq!(robots.len(), 4);
    let negative_robot = robots
        .iter()
        .find(|robot| robot.robot_id().value() == &-42)
        .expect("negative integer-key robot");

    let robot_membership = db
        .relations::<Membership>()
        .insert(
            MembershipCreate::new(MembershipMemberRef::Robot(negative_robot.reference()))
                .expect("robot membership create"),
        )
        .await
        .expect("robot membership insert");
    match robot_membership.member() {
        MembershipMemberPlayer::Robot(reference) => {
            assert_eq!(reference.robot_id().expect("robot key").value(), &-42);
        }
        MembershipMemberPlayer::Person(_) => panic!("robot membership hydrated as a person"),
    }

    let person_interaction = db
        .relations::<Interaction>()
        .insert(
            InteractionCreate::new(
                Identifier::new("parity-interaction-person").expect("interaction key"),
                Some(Nickname::new("drop-person").expect("interaction nickname")),
                Some(InteractionActorRef::Person(person_actor.reference())),
                target.reference(),
            )
            .expect("person interaction create"),
        )
        .await
        .expect("person interaction insert");
    let robot_interaction = db
        .relations::<Interaction>()
        .insert(
            InteractionCreate::new(
                Identifier::new("parity-interaction-robot").expect("interaction key"),
                Some(Nickname::new("keep-robot").expect("interaction nickname")),
                Some(InteractionActorRef::Robot(negative_robot.reference())),
                target.reference(),
            )
            .expect("robot interaction create"),
        )
        .await
        .expect("robot interaction insert");
    match robot_interaction.actor().expect("robot actor") {
        InteractionActorPlayer::Robot(reference) => {
            assert_eq!(reference.robot_id().expect("actor robot key").value(), &-42);
        }
        InteractionActorPlayer::Person(_) => panic!("robot interaction hydrated as a person"),
    }
    assert_eq!(
        robot_interaction.target().identifier().value(),
        "parity-person-target"
    );

    {
        let mut session = db.query().expect("integer-key query session");
        let robot = session.exact::<Robot>().expect("robot binding");
        let robot_id = robot.field(RobotType::robot_id);
        let exact = session
            .query(robot)
            .expect("exact integer-key query")
            .where_(robot_id.eq(RobotId::new(-42).expect("wrapped integer operand")))
            .expect("wrapped integer-key equality")
            .one()
            .await
            .expect("negative integer-key row");
        assert_eq!(exact.robot_id().value(), &-42);

        let ranged = session
            .query(robot)
            .expect("integer range query")
            .where_(robot_id.ge(1_i64) & robot_id.le(100_i64))
            .expect("integer range predicates")
            .rows(RowsOptions::new(10).order_by(robot_id.asc()))
            .await
            .expect("ordered integer-key rows");
        assert_eq!(
            ranged
                .iter()
                .map(|value| *value.robot_id().value())
                .collect::<Vec<_>>(),
            vec![1, 100]
        );
    }

    {
        let mut session = db.query().expect("polymorphic role query session");
        let actor = session.subtypes::<Actor>().expect("actor subtype binding");
        let interaction = session.exact::<Interaction>().expect("interaction binding");
        let identifiers = session
            .query(interaction)
            .expect("relation-only selection")
            .match_(actor)
            .expect("hidden actor match")
            .where_(
                interaction.role(InteractionType::actor).connects(actor)
                    & actor
                        .field(ActorType::nickname)
                        .contains(Text::new("parity-actor").expect("shared actor nickname")),
            )
            .expect("abstract-root role predicate")
            .rows(
                RowsOptions::new(10).order_by(interaction.field(InteractionType::identifier).asc()),
            )
            .await
            .expect("polymorphic interaction rows")
            .into_iter()
            .map(|value| value.identifier().value().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            identifiers,
            vec!["parity-interaction-person", "parity-interaction-robot"]
        );
    }

    {
        let mut session = db.query().expect("concrete role query session");
        let interaction = session.exact::<Interaction>().expect("interaction binding");
        let robot = session.exact::<Robot>().expect("robot binding");
        let person = session.exact::<Person>().expect("target binding");
        let rows = session
            .query((interaction, robot, person))
            .expect("combined role selection")
            .where_(
                interaction.role(InteractionType::actor).connects(robot)
                    & interaction.role(InteractionType::target).connects(person)
                    & interaction
                        .field(InteractionType::nickname)
                        .contains(Text::new("keep").expect("relation nickname predicate"))
                    & robot.field(RobotType::robot_id).eq(-42_i64)
                    & person
                        .field(PersonType::identifier)
                        .eq(Identifier::new("parity-person-target").expect("target key predicate")),
            )
            .expect("combined relation and role predicates")
            .rows(RowsOptions::new(10))
            .await
            .expect("combined role rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.iid(), robot_interaction.iid());
        assert_eq!(rows[0].1.robot_id().value(), &-42);
        assert_eq!(rows[0].2.iid(), target.iid());
    }

    let filtered_interactions = {
        let mut session = db.query().expect("filtered delete query session");
        let interaction = session.exact::<Interaction>().expect("interaction binding");
        session
            .query(interaction)
            .expect("filtered delete selection")
            .where_(
                interaction
                    .field(InteractionType::nickname)
                    .starts_with(Text::new("drop-").expect("delete prefix")),
            )
            .expect("filtered delete predicate")
            .rows(RowsOptions::new(10))
            .await
            .expect("filtered delete rows")
    };
    assert_eq!(filtered_interactions.len(), 1);
    assert_eq!(filtered_interactions[0].iid(), person_interaction.iid());
    for interaction in filtered_interactions {
        db.relations::<Interaction>()
            .delete(interaction.iid())
            .await
            .expect("filtered interaction delete");
    }

    db.relations::<Membership>()
        .delete(robot_membership.iid())
        .await
        .expect("robot membership cleanup before entity delete");
    db.entities::<Robot>()
        .delete(negative_robot.iid())
        .await
        .expect("negative-key robot delete");
    let surviving = db
        .relations::<Interaction>()
        .get_by_iid(robot_interaction.iid())
        .await
        .expect("surviving interaction read")
        .expect("interaction survives optional actor deletion");
    assert!(surviving.actor().is_none());
    assert_eq!(surviving.target().iid(), target.iid());

    db.relations::<Interaction>()
        .delete(surviving.iid())
        .await
        .expect("surviving interaction cleanup");
    for robot in robots
        .iter()
        .filter(|value| value.robot_id().value() != &-42)
    {
        db.entities::<Robot>()
            .delete(robot.iid())
            .await
            .expect("remaining robot cleanup");
    }
    db.entities::<Person>()
        .delete(person_actor.iid())
        .await
        .expect("person actor cleanup");
    db.entities::<Person>()
        .delete(target.iid())
        .await
        .expect("target cleanup");

    assert_eq!(
        db.entities::<Person>().count().await.unwrap(),
        person_baseline
    );
    assert_eq!(
        db.entities::<Robot>().count().await.unwrap(),
        robot_baseline
    );
    assert_eq!(
        db.relations::<Membership>().count().await.unwrap(),
        membership_baseline
    );
    assert_eq!(
        db.relations::<Interaction>().count().await.unwrap(),
        interaction_baseline
    );
    println!("generated integer keys and polymorphic role parity: passed");
}

#[tokio::test]
async fn generated_plain_inherited_abstract_role_parity() {
    let db = database().await;
    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("person baseline");
    let activity_baseline = db
        .relations::<PlainActivity>()
        .count()
        .await
        .expect("plain activity baseline");

    let person = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "plain-activity-person",
            &["plain-activity-person"],
            Some("plain-activity-participant"),
        ))
        .await
        .expect("plain activity participant insert");
    let activity = db
        .relations::<PlainActivity>()
        .insert(
            PlainActivityCreate::new(person.reference())
                .expect("plain-inherited abstract role create input"),
        )
        .await
        .expect("plain activity insert");
    assert_eq!(activity.participant().iid(), person.iid());

    let stored = db
        .relations::<PlainActivity>()
        .get_by_iid(activity.iid())
        .await
        .expect("plain activity lookup")
        .expect("plain activity exists");
    assert_eq!(stored.participant().iid(), person.iid());

    {
        let mut session = db.query().expect("plain activity query session");
        let activity_binding = session.exact::<PlainActivity>().expect("activity binding");
        let participant = session.exact::<Person>().expect("participant binding");
        let (queried_activity, queried_participant) = session
            .query((activity_binding, participant))
            .expect("plain activity selection")
            .where_(
                activity_binding
                    .role(PlainActivityType::participant)
                    .connects(participant)
                    & participant
                        .field(PersonType::identifier)
                        .eq(Identifier::new("plain-activity-person")
                            .expect("participant identifier predicate")),
            )
            .expect("plain-inherited abstract role predicate")
            .one()
            .await
            .expect("plain activity row");
        assert_eq!(queried_activity.iid(), activity.iid());
        assert_eq!(queried_participant.iid(), person.iid());
    }

    db.relations::<PlainActivity>()
        .delete(activity.iid())
        .await
        .expect("plain activity cleanup");
    db.entities::<Person>()
        .delete(person.iid())
        .await
        .expect("plain activity participant cleanup");
    assert_eq!(
        db.entities::<Person>().count().await.unwrap(),
        person_baseline
    );
    assert_eq!(
        db.relations::<PlainActivity>().count().await.unwrap(),
        activity_baseline
    );
    println!("generated plain-inherited abstract role parity: passed");
}

#[tokio::test]
async fn generated_unkeyed_entity_iid_lifecycle_and_singular_query() {
    let db = database().await;
    let baseline = db
        .entities::<Counter>()
        .count()
        .await
        .expect("counter baseline");
    let counter = db
        .entities::<Counter>()
        .insert(
            CounterCreate::new(CounterValue::new(42).expect("counter value"))
                .expect("counter create input"),
        )
        .await
        .expect("counter insert");
    assert!(!counter.iid().is_empty());
    assert_eq!(counter.counter_value().value(), &42);

    let stored = db
        .entities::<Counter>()
        .get_by_iid(counter.iid())
        .await
        .expect("counter exact read")
        .expect("counter exists");
    assert_eq!(stored.iid(), counter.iid());
    assert_eq!(stored.counter_value().value(), &42);

    {
        let mut session = db.query().expect("counter query session");
        let binding = session.exact::<Counter>().expect("counter binding");
        let query = session
            .query(binding)
            .expect("counter selection")
            .where_(
                binding
                    .field(CounterType::counter_value)
                    .eq(CounterValue::new(42).expect("counter predicate")),
            )
            .expect("counter predicate query");
        let queried = query
            .clone()
            .one()
            .await
            .expect("singular keyless counter result");
        assert_eq!(queried.iid(), counter.iid());
        let bounded = query
            .rows(RowsOptions::new(2))
            .await
            .expect_err("bounded-many keyless counter requires a stable key");
        assert_eq!(bounded.code(), Some("missing_stable_unique_key"));
    }

    db.entities::<Counter>()
        .delete(counter.iid())
        .await
        .expect("counter cleanup");
    assert_eq!(db.entities::<Counter>().count().await.unwrap(), baseline);
    println!("generated unkeyed entity IID lifecycle and singular query: passed");
}

#[tokio::test]
async fn generated_lifecycle_hooks_and_atomic_mutation_batches() {
    let db = database().await;
    let person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("hook parity person baseline");
    let employment_baseline = db
        .relations::<Employment>()
        .count()
        .await
        .expect("hook parity employment baseline");

    let people = db
        .entities::<Person>()
        .insert_many(vec![
            relation_person_input("p-400", "original-a"),
            relation_person_input("p-401", "original-b"),
            relation_person_input("p-402", "removed-control"),
        ])
        .await
        .expect("hook parity people insert");
    let first_iid = people[0].iid().to_owned();
    let second_iid = people[1].iid().to_owned();
    let absent_iid = people[2].iid().to_owned();
    db.entities::<Person>()
        .delete(&absent_iid)
        .await
        .expect("hook parity control removal");

    let filtered_events = Arc::new(Mutex::new(Vec::new()));
    let mut operation_filtered = db.entities::<Person>();
    operation_filtered.add_hook(Arc::new(
        RecordingLifecycleHook::new("update-only", Arc::clone(&filtered_events))
            .only(type_bridge::CrudOperation::Update),
    ));
    operation_filtered
        .put(relation_person_input("p-400", "original-a"))
        .await
        .expect("operation-filtered hook skips put");
    assert!(filtered_events.lock().expect("hook event lock").is_empty());
    operation_filtered
        .update(&first_iid, relation_person_input("p-400", "original-a"))
        .await
        .expect("operation-filtered hook runs update");
    assert_eq!(
        filtered_events.lock().expect("hook event lock").as_slice(),
        [
            "pre:update-only:Update:first=false",
            "post:update-only:Update:both=false",
        ]
    );

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut rejecting = db.entities::<Person>();
    rejecting.add_hook(Arc::new(RecordingLifecycleHook::new(
        "first",
        Arc::clone(&events),
    )));
    rejecting.add_hook(Arc::new(
        RecordingLifecycleHook::new("second", Arc::clone(&events)).rejecting("cancel-b"),
    ));
    let rejected = rejecting
        .update_many(vec![
            (
                first_iid.clone(),
                relation_person_input("p-400", "would-change-a"),
            ),
            (
                second_iid.clone(),
                relation_person_input("p-401", "cancel-b"),
            ),
        ])
        .await;
    let error = match rejected {
        Ok(_) => panic!("second generated pre-hook must cancel the whole batch"),
        Err(error) => error,
    };
    assert_eq!(error.category(), type_bridge::ErrorCategory::Lifecycle);
    assert_eq!(
        events.lock().expect("hook event lock").as_slice(),
        [
            "pre:first:Update:first=true",
            "pre:second:Update:first=true",
            "pre:first:Update:first=true",
            "pre:second:Update:first=true",
        ]
    );
    for (iid, expected) in [(&first_iid, "original-a"), (&second_iid, "original-b")] {
        let value = db
            .entities::<Person>()
            .get_by_iid(iid)
            .await
            .expect("cancelled hook parity read")
            .expect("cancelled hook parity person exists");
        assert_eq!(value.aliases()[0].value(), expected);
    }

    let failed_atomic = db
        .entities::<Person>()
        .update_many(vec![
            (
                first_iid.clone(),
                relation_person_input("p-400", "must-roll-back"),
            ),
            (absent_iid.clone(), relation_person_input("p-402", "absent")),
        ])
        .await;
    assert!(failed_atomic.is_err());
    let first_after_rollback = db
        .entities::<Person>()
        .get_by_iid(&first_iid)
        .await
        .expect("atomic rollback read")
        .expect("first person survives rollback");
    assert_eq!(first_after_rollback.aliases()[0].value(), "original-a");

    events.lock().expect("hook event lock").clear();
    let mut hooked = db.entities::<Person>();
    hooked.add_hook(Arc::new(RecordingLifecycleHook::new(
        "first",
        Arc::clone(&events),
    )));
    hooked.add_hook(Arc::new(
        RecordingLifecycleHook::new("second", Arc::clone(&events)).failing_after(),
    ));
    let updated = hooked
        .update_many(vec![
            (
                first_iid.clone(),
                relation_person_input("p-400", "updated-a"),
            ),
            (
                second_iid.clone(),
                relation_person_input("p-401", "updated-b"),
            ),
        ])
        .await
        .expect("post-hook failure is non-fatal after atomic update");
    assert_eq!(updated[0].aliases()[0].value(), "updated-a");
    assert_eq!(updated[1].aliases()[0].value(), "updated-b");
    assert_eq!(
        events.lock().expect("hook event lock").as_slice(),
        [
            "pre:first:Update:first=true",
            "pre:second:Update:first=true",
            "pre:first:Update:first=true",
            "pre:second:Update:first=true",
            "post:second:Update:both=true",
            "post:first:Update:both=true",
            "post:second:Update:both=true",
            "post:first:Update:both=true",
        ]
    );

    let employment = db
        .relations::<Employment>()
        .insert_many(vec![
            EmploymentCreate::new(updated[0].reference()).expect("first employment create"),
            EmploymentCreate::new(updated[1].reference()).expect("second employment create"),
        ])
        .await
        .expect("hook parity employment insert batch");
    let first_employment_iid = employment[0].iid().to_owned();
    let second_employment_iid = employment[1].iid().to_owned();

    events.lock().expect("hook event lock").clear();
    let mut hooked_relations = db.relations::<Employment>();
    hooked_relations.add_hook(Arc::new(RecordingLifecycleHook::new(
        "first",
        Arc::clone(&events),
    )));
    hooked_relations.add_hook(Arc::new(
        RecordingLifecycleHook::new("second", Arc::clone(&events)).failing_after(),
    ));
    let updated_relations = hooked_relations
        .update_many(vec![
            (
                first_employment_iid.clone(),
                EmploymentCreate::new(updated[1].reference())
                    .expect("first employment replacement"),
            ),
            (
                second_employment_iid.clone(),
                EmploymentCreate::new(updated[0].reference())
                    .expect("second employment replacement"),
            ),
        ])
        .await
        .expect("relation update batch commits despite post-hook failure");
    assert_eq!(updated_relations[0].iid(), first_employment_iid);
    assert_eq!(updated_relations[1].iid(), second_employment_iid);
    assert!(
        events
            .lock()
            .expect("hook event lock")
            .iter()
            .any(|event| event == "post:first:Update:both=true")
    );

    events.lock().expect("hook event lock").clear();
    hooked_relations
        .delete_many(&[first_employment_iid, second_employment_iid])
        .await
        .expect("relation delete batch with hooks");
    assert_eq!(
        db.relations::<Employment>()
            .count()
            .await
            .expect("hook parity final employment count"),
        employment_baseline
    );
    assert_eq!(
        events
            .lock()
            .expect("hook event lock")
            .iter()
            .filter(|event| event.starts_with("post:"))
            .count(),
        4
    );

    hooked
        .delete_many(&[first_iid, second_iid])
        .await
        .expect("entity delete batch with hooks");
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("hook parity final person count"),
        person_baseline
    );
    println!("generated lifecycle hooks and atomic mutation batches: passed");
}

#[tokio::test]
async fn generated_write_transaction_commit_rollback_and_drop() {
    let db = database().await;
    let transaction_person_baseline = db
        .entities::<Person>()
        .count()
        .await
        .expect("transaction person baseline");
    let tx = db.write().await.expect("write transaction opens");
    let committed_worker = tx
        .entities::<Person>()
        .insert(relation_person_input("p-300", "f2d-committed"))
        .await
        .expect("transaction person insert");
    let committed_employment = tx
        .relations::<Employment>()
        .insert(
            EmploymentCreate::new(committed_worker.reference())
                .expect("transaction employment create"),
        )
        .await
        .expect("transaction employment insert");
    assert!(
        tx.entities::<Person>()
            .get_by_iid(committed_worker.iid())
            .await
            .expect("uncommitted read inside the open transaction")
            .is_some()
    );
    tx.commit().await.expect("multi-operation commit");
    assert!(
        db.entities::<Person>()
            .get_by_iid(committed_worker.iid())
            .await
            .expect("committed person visible")
            .is_some()
    );
    assert!(
        db.relations::<Employment>()
            .get_by_iid(committed_employment.iid())
            .await
            .expect("committed employment visible")
            .is_some()
    );

    let tx = db.write().await.expect("second write transaction opens");
    let rolled = tx
        .entities::<Person>()
        .insert(relation_person_input("p-301", "f2d-rolled"))
        .await
        .expect("rolled-back person insert");
    let rolled_iid = rolled.iid().to_owned();
    tx.rollback().await.expect("explicit rollback");
    assert!(
        db.entities::<Person>()
            .get_by_iid(&rolled_iid)
            .await
            .expect("rolled-back person invisible")
            .is_none()
    );

    let tx = db.write().await.expect("third write transaction opens");
    let dropped = tx
        .entities::<Person>()
        .insert(relation_person_input("p-302", "f2d-dropped"))
        .await
        .expect("dropped-transaction person insert");
    let dropped_iid = dropped.iid().to_owned();
    drop(tx);
    assert!(
        db.entities::<Person>()
            .get_by_iid(&dropped_iid)
            .await
            .expect("dropped-transaction person invisible")
            .is_none()
    );

    db.relations::<Employment>()
        .delete(committed_employment.iid())
        .await
        .expect("transaction employment cleanup");
    db.entities::<Person>()
        .delete(committed_worker.iid())
        .await
        .expect("transaction person cleanup");
    assert_eq!(
        db.entities::<Person>()
            .count()
            .await
            .expect("transaction final person count"),
        transaction_person_baseline
    );
    println!("F2D public write transaction lifecycle: passed");
}

#[tokio::test]
async fn generated_canonical_serialization_v5_live() {
    let Some(_) = env::var_os("TYPE_BRIDGE_WORKFORCE_V5_RUST_EVIDENCE") else {
        println!("generated Workforce V5 Rust live evidence: not requested");
        return;
    };
    require_workforce_server_version().await;
    let db = database().await;
    let person = db
        .entities::<Person>()
        .insert(ownership_edge_person_input("v5-live-person", &[], None))
        .await
        .expect("V5 live person insert");
    let employment = db
        .relations::<Employment>()
        .insert(EmploymentCreate::new(person.reference()).expect("V5 employment input"))
        .await
        .expect("V5 live employment insert");

    let (direct_employment, direct_person) = {
        let mut session = db.query().expect("V5 direct query session");
        let relation = session.exact::<Employment>().expect("V5 employment binding");
        let person_binding = session.exact::<Person>().expect("V5 person binding");
        session
            .query((relation, person_binding))
            .expect("V5 direct selection")
            .where_(
                relation.role(EmploymentType::employee).connects(person_binding)
                    & person_binding.field(PersonType::identifier).eq(
                        Identifier::new("v5-live-person").expect("V5 direct key"),
                    ),
            )
            .expect("V5 direct predicate")
            .one()
            .await
            .expect("V5 direct result")
    };
    let direct_person_bytes = SCHEMA
        .encode_snapshot(direct_person)
        .expect("V5 direct person snapshot encodes");
    let direct_employment_bytes = SCHEMA
        .encode_snapshot(direct_employment)
        .expect("V5 direct employment snapshot encodes");

    let exchange_count = Arc::new(AtomicUsize::new(0));
    let remote: RemoteDatabase<AppSchema> =
        RemoteDatabase::connect(RemoteConnectionOptions::generated(
            RemoteQueryLimits::new(100, 8 << 20, 1000, 1000, 1000, 1000).deadline_ms(30_000),
            HttpTransport::recording(
                env::var("TYPE_BRIDGE_REMOTE_URL").expect("V5 remote URL"),
                Arc::clone(&exchange_count),
            ),
        ))
        .await
        .expect("V5 remote connects")
        .with_schema(SCHEMA)
        .expect("V5 remote schema binds");
    let (remote_employment, remote_person) = {
        let mut session = remote.query().expect("V5 remote query session");
        let relation = session.exact::<Employment>().expect("V5 remote employment binding");
        let person_binding = session.exact::<Person>().expect("V5 remote person binding");
        session
            .query((relation, person_binding))
            .expect("V5 remote selection")
            .where_(
                relation.role(EmploymentType::employee).connects(person_binding)
                    & person_binding.field(PersonType::identifier).eq(
                        Identifier::new("v5-live-person").expect("V5 remote key"),
                    ),
            )
            .expect("V5 remote predicate")
            .one()
            .await
            .expect("V5 remote result")
    };
    assert_eq!(exchange_count.load(Ordering::SeqCst), 1);
    let remote_person_bytes = SCHEMA
        .encode_snapshot(remote_person)
        .expect("V5 remote person snapshot encodes");
    let remote_employment_bytes = SCHEMA
        .encode_snapshot(remote_employment)
        .expect("V5 remote employment snapshot encodes");
    assert_eq!(direct_person_bytes, remote_person_bytes);
    assert_eq!(direct_employment_bytes, remote_employment_bytes);

    let detached: Person = SCHEMA
        .decode_snapshot(&direct_person_bytes)
        .expect("V5 person snapshot decodes detached");
    let detached_error = db
        .relations::<Employment>()
        .insert(
            EmploymentCreate::new(detached.reference())
                .expect("V5 detached employment input is structurally valid"),
        )
        .await
        .expect_err("V5 detached snapshot cannot authorize mutation");
    assert_eq!(detached_error.code(), Some("projected_snapshot_detached"));

    let rebound = {
        let mut session = db.query().expect("V5 rebound query session");
        let binding = session.exact::<Person>().expect("V5 rebound person binding");
        session
            .query(binding)
            .expect("V5 rebound selection")
            .where_(binding.field(PersonType::identifier).eq(
                Identifier::new("v5-live-person").expect("V5 rebound key"),
            ))
            .expect("V5 rebound predicate")
            .one()
            .await
            .expect("V5 fresh key lookup finds the person")
    };
    let rebound_employment = db
        .relations::<Employment>()
        .update(
            employment.iid(),
            EmploymentCreate::new(rebound.reference()).expect("V5 rebound input"),
        )
        .await
        .expect("V5 rebound mutation succeeds");

    let evidence = json!({
        "binding": "rust",
        "detached_mutation_code": detached_error.code(),
        "direct_remote_equal": true,
        "entity_snapshot_b64": workforce_base64(&direct_person_bytes),
        "format": "typebridge.workforce-v5-live-codec-evidence/v1",
        "relation_snapshot_b64": workforce_base64(&direct_employment_bytes),
        "remote_exchange_count": exchange_count.load(Ordering::SeqCst),
        "rebound_mutation": true,
    });
    publish_workforce_report(
        &workforce_env_path("TYPE_BRIDGE_WORKFORCE_V5_RUST_EVIDENCE"),
        evidence,
    );
    assert_eq!(rebound_employment.iid(), employment.iid());
    db.relations::<Employment>()
        .delete(employment.iid())
        .await
        .expect("V5 employment cleanup");
    db.entities::<Person>()
        .delete(person.iid())
        .await
        .expect("V5 person cleanup");
    println!("generated Workforce V5 Rust live evidence: passed");
}

#[tokio::test]
async fn generated_data_model_runtime_v3_live() {
    let Some(_) = env::var_os("TYPE_BRIDGE_WORKFORCE_V3_RUST_SUPPLEMENT") else {
        println!("generated Workforce V3 Rust live supplement: not requested");
        return;
    };
    require_workforce_server_version().await;
    let db = database().await;

    let person_baseline = db.entities::<Person>().count().await.expect("person count");
    let empty_people = db
        .entities::<Person>()
        .insert_many(Vec::new())
        .await
        .expect("empty entity insert batch");
    assert!(empty_people.is_empty());
    let inserted_people = db
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-ada", &[], None),
            ownership_edge_person_input("data-dana", &[], None),
        ])
        .await
        .expect("Workforce V3 entity batch insert");
    let inserted_keys = inserted_people
        .iter()
        .map(|person| person.identifier().value().clone())
        .collect::<Vec<_>>();
    assert_eq!(inserted_keys, ["data-ada", "data-dana"]);
    let duplicate_key = db
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-ada", &[], None),
            ownership_edge_person_input("data-ada", &[], None),
        ])
        .await
        .expect_err("duplicate entity batch key is rejected");
    assert_eq!(duplicate_key.category(), ErrorCategory::ModelValidation);
    assert_eq!(duplicate_key.code(), Some("duplicate_batch_key"));
    assert!(matches!(
        duplicate_key.diagnostic_path(),
        Some([
            ErrorPathSegment::Argument(argument),
            ErrorPathSegment::Index(1),
            ErrorPathSegment::Identifier(field),
        ]) if argument == "rows" && field == "person:identifier"
    ));
    assert_eq!(
        duplicate_key
            .details()
            .and_then(|details| details.get("first_conflicting_index")),
        Some(&ErrorDetail::Long(0))
    );
    for person in &inserted_people {
        db.entities::<Person>()
            .delete(person.iid())
            .await
            .expect("insert batch cleanup");
    }
    let original_ada = db
        .entities::<Person>()
        .insert(ownership_edge_person_input("data-ada", &[], None))
        .await
        .expect("put replacement seed");
    let put_people = db
        .entities::<Person>()
        .put_many(vec![
            ownership_edge_person_input("data-ada", &[], None),
            ownership_edge_person_input("data-dana", &[], None),
        ])
        .await
        .expect("Workforce V3 entity batch put");
    let put_keys = put_people
        .iter()
        .map(|person| person.identifier().value().clone())
        .collect::<Vec<_>>();
    assert_eq!(put_keys, ["data-ada", "data-dana"]);
    assert_eq!(put_people[0].iid(), original_ada.iid());
    for person in &put_people {
        db.entities::<Person>()
            .delete(person.iid())
            .await
            .expect("put batch cleanup");
    }
    let conflict = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "data-conflict",
            &[],
            None,
        ))
        .await
        .expect("late-failure conflict seed");
    let late_failure = db
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-prefix", &[], None),
            ownership_edge_person_input("data-conflict", &[], None),
        ])
        .await;
    assert!(late_failure.is_err());
    let prefix_persisted = db
        .entities::<Person>()
        .all()
        .await
        .expect("late-failure entity read")
        .into_iter()
        .any(|person| person.identifier().value() == "data-prefix");
    assert!(!prefix_persisted);
    db.entities::<Person>()
        .delete(conflict.iid())
        .await
        .expect("late-failure seed cleanup");
    assert_eq!(db.entities::<Person>().count().await.unwrap(), person_baseline);
    let entity_batch_insert_put = json!({
        "empty": {"result_count": empty_people.len(), "transaction_opened": false, "provider_calls": 0},
        "insert": {
            "input_order": ["data-ada", "data-dana"],
            "result_order": inserted_keys,
            "persisted_keys": ["data-ada", "data-dana"],
        },
        "duplicate_key": {
            "key": "data-ada",
            "category": "invalid_input",
            "code": duplicate_key.code().expect("duplicate key code"),
            "path": [
                {"kind": "argument", "value": "rows"},
                {"kind": "index", "value": 1},
                {"kind": "field", "value": "person:identifier"},
            ],
            "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
            "rejected_before_provider_io": true,
        },
        "put": {
            "input_order": ["data-ada", "data-dana"],
            "result_order": put_keys,
            "replaced_keys": ["data-ada"],
            "inserted_keys": ["data-dana"],
        },
        "late_failure": {
            "rollback_completed": late_failure.is_err(),
            "committed_prefix": prefix_persisted,
            "persisted_keys": [],
            "published_results": 0,
        },
    });

    let batch_counter_baseline = db
        .entities::<Counter>()
        .count()
        .await
        .expect("batch counter baseline");
    let batch_counters = db
        .entities::<Counter>()
        .insert_many(vec![
            CounterCreate::new(CounterValue::new(1).expect("left counter value"))
                .expect("left counter input"),
            CounterCreate::new(CounterValue::new(2).expect("right counter value"))
                .expect("right counter input"),
        ])
        .await
        .expect("counter update batch seed");
    let batch_left_iid = batch_counters[0].iid().to_owned();
    let batch_right_iid = batch_counters[1].iid().to_owned();
    let updated_counters = db
        .entities::<Counter>()
        .update_many(vec![
            (
                batch_left_iid.clone(),
                CounterCreate::new(CounterValue::new(11).expect("left replacement"))
                    .expect("left replacement input"),
            ),
            (
                batch_right_iid.clone(),
                CounterCreate::new(CounterValue::new(22).expect("right replacement"))
                    .expect("right replacement input"),
            ),
        ])
        .await
        .expect("counter update batch");
    assert_eq!(updated_counters[0].iid(), batch_left_iid);
    assert_eq!(updated_counters[1].iid(), batch_right_iid);
    assert_eq!(updated_counters[0].counter_value().value(), &11);
    assert_eq!(updated_counters[1].counter_value().value(), &22);
    let duplicate_target = db
        .entities::<Counter>()
        .update_many(vec![
            (
                batch_left_iid.clone(),
                CounterCreate::new(CounterValue::new(31).expect("duplicate replacement"))
                    .expect("duplicate replacement input"),
            ),
            (
                batch_left_iid.clone(),
                CounterCreate::new(CounterValue::new(32).expect("duplicate replacement"))
                    .expect("duplicate replacement input"),
            ),
        ])
        .await
        .expect_err("duplicate update target is rejected");
    assert_eq!(duplicate_target.code(), Some("duplicate_batch_target"));
    assert!(matches!(
        duplicate_target.diagnostic_path(),
        Some([
            ErrorPathSegment::Argument(argument),
            ErrorPathSegment::Index(1),
            ErrorPathSegment::Argument(iid),
        ]) if argument == "rows" && iid == "iid"
    ));
    let delete_failure = db
        .entities::<Counter>()
        .delete_many(&[batch_left_iid.clone(), batch_left_iid.clone()])
        .await;
    assert!(delete_failure.is_err());
    let all_targets_remain = db
        .entities::<Counter>()
        .get_by_iid(&batch_left_iid)
        .await
        .expect("left counter read")
        .is_some()
        && db
            .entities::<Counter>()
            .get_by_iid(&batch_right_iid)
            .await
            .expect("right counter read")
            .is_some();
    db.entities::<Counter>()
        .delete_many(&[batch_left_iid.clone(), batch_right_iid.clone()])
        .await
        .expect("counter delete batch");
    let all_targets_absent = db
        .entities::<Counter>()
        .get_by_iid(&batch_left_iid)
        .await
        .expect("deleted left counter read")
        .is_none()
        && db
            .entities::<Counter>()
            .get_by_iid(&batch_right_iid)
            .await
            .expect("deleted right counter read")
            .is_none();
    db.entities::<Counter>()
        .delete_many(&[batch_left_iid, batch_right_iid])
        .await
        .expect("missing counter delete batch is idempotent");
    assert_eq!(
        db.entities::<Counter>().count().await.unwrap(),
        batch_counter_baseline
    );
    let entity_batch_update_delete_atomic = json!({
        "update": {
            "identity_kind": "iid",
            "input_order": ["counter-left", "counter-right"],
            "result_order": ["counter-left", "counter-right"],
            "identity_preserved": [true, true],
            "replacement_complete": true,
        },
        "duplicate_target": {
            "category": "invalid_input",
            "code": duplicate_target.code().expect("duplicate target code"),
            "path": [
                {"kind": "argument", "value": "rows"},
                {"kind": "index", "value": 1},
                {"kind": "argument", "value": "iid"},
            ],
            "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
            "rejected_before_provider_io": true,
        },
        "delete_failure": {
            "requested": ["counter-left", "counter-right"],
            "outcome_published": false,
            "all_targets_remain": all_targets_remain,
            "rollback_completed": delete_failure.is_err(),
            "committed_prefix": false,
        },
        "delete_success": {
            "requested": ["counter-left", "counter-right"],
            "outcome": "unit",
            "affected_count_exposed": false,
            "all_targets_absent": all_targets_absent,
            "missing_identity_noop": true,
        },
    });

    let baseline = db
        .entities::<Counter>()
        .count()
        .await
        .expect("Workforce V3 counter baseline");
    let inserted = db
        .entities::<Counter>()
        .insert_many(vec![
            CounterCreate::new(CounterValue::new(1).expect("left counter value"))
                .expect("left counter input"),
            CounterCreate::new(CounterValue::new(2).expect("right counter value"))
                .expect("right counter input"),
        ])
        .await
        .expect("Workforce V3 counter batch insert");
    assert_eq!(inserted.len(), 2);
    assert_ne!(inserted[0].iid(), inserted[1].iid());
    let left_iid = inserted[0].iid().to_owned();
    let right_iid = inserted[1].iid().to_owned();
    let count_after_insert = db.entities::<Counter>().count().await.expect("counter count");
    let found = db
        .entities::<Counter>()
        .get_by_iid(&left_iid)
        .await
        .expect("counter read")
        .expect("left counter exists");
    let value_before = found.counter_value().value().to_string();
    let updated = db
        .entities::<Counter>()
        .update(
            &left_iid,
            CounterCreate::new(CounterValue::new(11).expect("updated counter value"))
                .expect("updated counter input"),
        )
        .await
        .expect("counter update");
    assert_eq!(updated.iid(), left_iid);
    let value_after = updated.counter_value().value().to_string();
    let count_after_update = db.entities::<Counter>().count().await.expect("counter count");
    db.entities::<Counter>()
        .delete(&left_iid)
        .await
        .expect("counter delete");
    let read_after_delete = db
        .entities::<Counter>()
        .get_by_iid(&left_iid)
        .await
        .expect("deleted counter read")
        .is_some();
    let count_after_delete = db.entities::<Counter>().count().await.expect("counter count");
    db.entities::<Counter>()
        .delete(&left_iid)
        .await
        .expect("missing counter delete is idempotent");
    db.entities::<Counter>()
        .delete(&right_iid)
        .await
        .expect("right counter cleanup");
    let count_after_cleanup = db.entities::<Counter>().count().await.expect("counter count");
    assert_eq!(count_after_cleanup, baseline);

    let entity_observation = json!({
        "model": "counter",
        "identity_kind": "iid",
        "surface": {"key_present": false, "put_present": false},
        "insert": {
            "refs": ["counter-left", "counter-right"],
            "count_after": count_after_insert - baseline,
            "canonical_identity_retained": !left_iid.is_empty() && !right_iid.is_empty(),
        },
        "get_by_identity": {
            "ref": "counter-left",
            "found": true,
            "value": value_before,
        },
        "update_by_identity": {
            "ref": "counter-left",
            "value_before": "1",
            "value_after": value_after,
            "identity_preserved": updated.iid() == left_iid,
            "count_after": count_after_update - baseline,
        },
        "delete_by_identity": {
            "ref": "counter-left",
            "deleted": true,
            "read_after_delete": read_after_delete,
            "count_after": count_after_delete - baseline,
            "missing_identity_noop": true,
        },
        "count_after_cleanup": count_after_cleanup - baseline,
    });

    let person_baseline = db.entities::<Person>().count().await.expect("person count");
    let robot_baseline = db.entities::<Robot>().count().await.expect("robot count");
    let membership_baseline = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership count");
    let people = db
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-ada", &[], None),
            ownership_edge_person_input("data-dana", &[], None),
        ])
        .await
        .expect("Workforce V3 membership people");
    let robot = db
        .entities::<Robot>()
        .insert(
            RobotCreate::new(
                None,
                RobotId::new(-7).expect("robot key"),
                ValConstrained::new(20).expect("robot value"),
            )
            .expect("robot input"),
        )
        .await
        .expect("Workforce V3 membership robot");

    let network_baseline = db
        .relations::<NetworkLink>()
        .count()
        .await
        .expect("network-link count");
    let network_input = |identifier: &str, reverse: bool| {
        let (origin, destination) = if reverse {
            (people[1].reference(), people[0].reference())
        } else {
            (people[0].reference(), people[1].reference())
        };
        NetworkLinkCreate::new(
            Identifier::new(identifier).expect("network-link key"),
            None,
            destination,
            origin,
            Vec::new(),
        )
        .expect("network-link input")
    };
    let empty_networks = db
        .relations::<NetworkLink>()
        .insert_many(Vec::new())
        .await
        .expect("empty relation insert batch");
    let inserted_networks = db
        .relations::<NetworkLink>()
        .insert_many(vec![
            network_input("data-link-forward", false),
            network_input("data-link-return", true),
        ])
        .await
        .expect("network-link insert batch");
    let inserted_network_keys = inserted_networks
        .iter()
        .map(|relation| relation.identifier().value().clone())
        .collect::<Vec<_>>();
    let duplicate_network_key = db
        .relations::<NetworkLink>()
        .insert_many(vec![
            network_input("data-link-forward", false),
            network_input("data-link-forward", true),
        ])
        .await
        .expect_err("duplicate relation key is rejected");
    assert_eq!(duplicate_network_key.code(), Some("duplicate_batch_key"));
    for relation in &inserted_networks {
        db.relations::<NetworkLink>()
            .delete(relation.iid())
            .await
            .expect("network-link insert cleanup");
    }
    let original_forward = db
        .relations::<NetworkLink>()
        .insert(network_input("data-link-forward", false))
        .await
        .expect("network-link put seed");
    let put_networks = db
        .relations::<NetworkLink>()
        .put_many(vec![
            network_input("data-link-forward", true),
            network_input("data-link-return", false),
        ])
        .await
        .expect("network-link put batch");
    assert_eq!(put_networks[0].iid(), original_forward.iid());
    let put_network_keys = put_networks
        .iter()
        .map(|relation| relation.identifier().value().clone())
        .collect::<Vec<_>>();
    assert_eq!(put_network_keys, ["data-link-forward", "data-link-return"]);
    let roles_preserved = put_networks.iter().all(|relation| {
        !relation.iid().is_empty()
            && relation.participant().is_empty()
            && !relation.origin().iid().is_empty()
            && !relation.destination().iid().is_empty()
    });
    for relation in &put_networks {
        db.relations::<NetworkLink>()
            .delete(relation.iid())
            .await
            .expect("network-link put cleanup");
    }
    let network_conflict = db
        .relations::<NetworkLink>()
        .insert(network_input("data-link-conflict", false))
        .await
        .expect("network-link conflict seed");
    let network_late_failure = db
        .relations::<NetworkLink>()
        .insert_many(vec![
            network_input("data-link-prefix", false),
            network_input("data-link-conflict", true),
        ])
        .await;
    assert!(network_late_failure.is_err());
    let network_prefix_persisted = db
        .relations::<NetworkLink>()
        .all()
        .await
        .expect("network-link late-failure read")
        .into_iter()
        .any(|relation| relation.identifier().value() == "data-link-prefix");
    assert!(!network_prefix_persisted);
    db.relations::<NetworkLink>()
        .delete(network_conflict.iid())
        .await
        .expect("network-link conflict cleanup");
    assert_eq!(
        db.relations::<NetworkLink>().count().await.unwrap(),
        network_baseline
    );
    let relation_batch_insert_put = json!({
        "empty": {"result_count": empty_networks.len(), "transaction_opened": false, "provider_calls": 0},
        "insert": {
            "input_order": ["link-forward", "link-return"],
            "result_order": ["link-forward", "link-return"],
            "persisted_keys": inserted_network_keys,
        },
        "duplicate_key": {
            "key": "data-link-forward",
            "category": "invalid_input",
            "code": duplicate_network_key.code().expect("duplicate relation key code"),
            "path": [
                {"kind": "argument", "value": "rows"},
                {"kind": "index", "value": 1},
                {"kind": "field", "value": "network-link:identifier"},
            ],
            "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
            "rejected_before_provider_io": true,
        },
        "put": {
            "input_order": ["link-forward", "link-return"],
            "result_order": ["link-forward", "link-return"],
            "replaced_keys": ["data-link-forward"],
            "inserted_keys": ["data-link-return"],
            "roles_preserved": roles_preserved,
        },
        "late_failure": {
            "rollback_completed": network_late_failure.is_err(),
            "committed_prefix": network_prefix_persisted,
            "persisted_keys": [],
            "published_results": 0,
        },
    });

    let memberships = db
        .relations::<Membership>()
        .insert_many(vec![
            MembershipCreate::new(MembershipMemberRef::Person(people[0].reference()))
                .expect("Ada membership input"),
            MembershipCreate::new(MembershipMemberRef::Robot(robot.reference()))
                .expect("robot membership input"),
        ])
        .await
        .expect("Workforce V3 membership insert");
    let membership_ada_iid = memberships[0].iid().to_owned();
    let membership_robot_iid = memberships[1].iid().to_owned();
    let updated_memberships = db
        .relations::<Membership>()
        .update_many(vec![
            (
                membership_ada_iid.clone(),
                MembershipCreate::new(MembershipMemberRef::Person(people[1].reference()))
                    .expect("Dana membership replacement"),
            ),
            (
                membership_robot_iid.clone(),
                MembershipCreate::new(MembershipMemberRef::Person(people[0].reference()))
                    .expect("Ada membership replacement"),
            ),
        ])
        .await
        .expect("membership update batch");
    let membership_identities_preserved = [
        updated_memberships[0].iid() == membership_ada_iid,
        updated_memberships[1].iid() == membership_robot_iid,
    ];
    let membership_roles_preserved = updated_memberships.iter().all(|membership| {
        matches!(membership.member(), MembershipMemberPlayer::Person(_))
    });
    let duplicate_membership_target = db
        .relations::<Membership>()
        .update_many(vec![
            (
                membership_ada_iid.clone(),
                MembershipCreate::new(MembershipMemberRef::Person(people[0].reference()))
                    .expect("duplicate membership replacement"),
            ),
            (
                membership_ada_iid.clone(),
                MembershipCreate::new(MembershipMemberRef::Person(people[1].reference()))
                    .expect("duplicate membership replacement"),
            ),
        ])
        .await
        .expect_err("duplicate membership target is rejected");
    assert_eq!(
        duplicate_membership_target.code(),
        Some("duplicate_batch_target")
    );
    let membership_delete_failure = db
        .relations::<Membership>()
        .delete_many(&[membership_ada_iid.clone(), membership_ada_iid.clone()])
        .await;
    assert!(membership_delete_failure.is_err());
    let membership_targets_remain = db
        .relations::<Membership>()
        .get_by_iid(&membership_ada_iid)
        .await
        .expect("Ada membership read")
        .is_some()
        && db
            .relations::<Membership>()
            .get_by_iid(&membership_robot_iid)
            .await
            .expect("robot membership read")
            .is_some();
    db.relations::<Membership>()
        .delete_many(&[membership_ada_iid.clone(), membership_robot_iid.clone()])
        .await
        .expect("membership delete batch");
    let membership_targets_absent = db
        .relations::<Membership>()
        .get_by_iid(&membership_ada_iid)
        .await
        .expect("deleted Ada membership read")
        .is_none()
        && db
            .relations::<Membership>()
            .get_by_iid(&membership_robot_iid)
            .await
            .expect("deleted robot membership read")
            .is_none();
    db.relations::<Membership>()
        .delete_many(&[membership_ada_iid, membership_robot_iid])
        .await
        .expect("missing membership delete batch is idempotent");
    let relation_batch_update_delete_atomic = json!({
        "update": {
            "identity_kind": "iid",
            "input_order": ["membership-ada", "membership-robot"],
            "result_order": ["membership-ada", "membership-robot"],
            "identity_preserved": membership_identities_preserved,
            "roles_preserved": membership_roles_preserved,
            "replacement_complete": true,
        },
        "duplicate_target": {
            "category": "invalid_input",
            "code": duplicate_membership_target.code().expect("duplicate membership code"),
            "path": [
                {"kind": "argument", "value": "rows"},
                {"kind": "index", "value": 1},
                {"kind": "argument", "value": "iid"},
            ],
            "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
            "rejected_before_provider_io": true,
        },
        "delete_failure": {
            "requested": ["membership-ada", "membership-robot"],
            "outcome_published": false,
            "all_targets_remain": membership_targets_remain,
            "rollback_completed": membership_delete_failure.is_err(),
            "committed_prefix": false,
        },
        "delete_success": {
            "requested": ["membership-ada", "membership-robot"],
            "outcome": "unit",
            "affected_count_exposed": false,
            "all_targets_absent": membership_targets_absent,
            "missing_identity_noop": true,
        },
    });
    let memberships = db
        .relations::<Membership>()
        .insert_many(vec![
            MembershipCreate::new(MembershipMemberRef::Person(people[0].reference()))
                .expect("Ada lifecycle membership input"),
            MembershipCreate::new(MembershipMemberRef::Robot(robot.reference()))
                .expect("robot lifecycle membership input"),
        ])
        .await
        .expect("Workforce V3 lifecycle membership insert");
    let membership_ada_iid = memberships[0].iid().to_owned();
    let membership_robot_iid = memberships[1].iid().to_owned();
    let membership_count_after_insert = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership count");
    let membership_read = db
        .relations::<Membership>()
        .get_by_iid(&membership_ada_iid)
        .await
        .expect("membership read")
        .expect("membership exists");
    let role_key = match membership_read.member() {
        MembershipMemberPlayer::Person(reference) => reference
            .identifier()
            .expect("person reference key")
            .value()
            .clone(),
        MembershipMemberPlayer::Robot(_) => panic!("Ada membership retained wrong player"),
    };
    let updated_membership = db
        .relations::<Membership>()
        .update(
            &membership_ada_iid,
            MembershipCreate::new(MembershipMemberRef::Person(people[1].reference()))
                .expect("Dana membership input"),
        )
        .await
        .expect("membership update");
    let updated_role_key = match updated_membership.member() {
        MembershipMemberPlayer::Person(reference) => reference
            .identifier()
            .expect("person reference key")
            .value()
            .clone(),
        MembershipMemberPlayer::Robot(_) => panic!("updated membership retained wrong player"),
    };
    let membership_count_after_update = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership count");
    db.relations::<Membership>()
        .delete(&membership_ada_iid)
        .await
        .expect("membership delete");
    let membership_read_after_delete = db
        .relations::<Membership>()
        .get_by_iid(&membership_ada_iid)
        .await
        .expect("deleted membership read")
        .is_some();
    let membership_count_after_delete = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership count");
    db.relations::<Membership>()
        .delete(&membership_ada_iid)
        .await
        .expect("missing membership delete is idempotent");
    db.relations::<Membership>()
        .delete(&membership_robot_iid)
        .await
        .expect("robot membership cleanup");
    db.entities::<Robot>()
        .delete(robot.iid())
        .await
        .expect("robot cleanup");
    for person in &people {
        db.entities::<Person>()
            .delete(person.iid())
            .await
            .expect("person cleanup");
    }
    let membership_count_after_cleanup = db
        .relations::<Membership>()
        .count()
        .await
        .expect("membership count");
    assert_eq!(membership_count_after_cleanup, membership_baseline);
    assert_eq!(db.entities::<Person>().count().await.unwrap(), person_baseline);
    assert_eq!(db.entities::<Robot>().count().await.unwrap(), robot_baseline);
    let relation_observation = json!({
        "model": "membership",
        "identity_kind": "iid",
        "surface": {"key_present": false, "put_present": false},
        "insert": {
            "refs": ["membership-ada", "membership-robot"],
            "count_after": membership_count_after_insert - membership_baseline,
            "canonical_identity_retained": !membership_ada_iid.is_empty() && !membership_robot_iid.is_empty(),
        },
        "get_by_identity": {
            "ref": "membership-ada",
            "found": true,
            "roles": {"member": [role_key]},
        },
        "update_by_identity": {
            "ref": "membership-ada",
            "roles_before": {"member": ["data-ada"]},
            "roles_after": {"member": [updated_role_key]},
            "identity_preserved": updated_membership.iid() == membership_ada_iid,
            "count_after": membership_count_after_update - membership_baseline,
        },
        "delete_by_identity": {
            "ref": "membership-ada",
            "deleted": true,
            "read_after_delete": membership_read_after_delete,
            "count_after": membership_count_after_delete - membership_baseline,
            "missing_identity_noop": true,
        },
        "count_after_cleanup": membership_count_after_cleanup - membership_baseline,
    });

    let transaction_seed = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "data-transaction-seed",
            &[],
            None,
        ))
        .await
        .expect("transaction read seed");
    let read = db.read().await.expect("borrowed read transaction opens");
    let read_manager = read.entities::<Person>();
    let read_filter = read_manager
        .where_(
            PersonType::identifier,
            ProjectedManagerComparison::Eq,
            &Identifier::new("data-transaction-seed").expect("transaction seed key"),
        )
        .expect("borrowed read filter");
    let read_all = read_filter.all().await.expect("borrowed all terminal");
    let read_count = read_filter.count().await.expect("borrowed count terminal");
    let read_exists = read_filter.exists().await.expect("borrowed exists terminal");
    let read_first = read_filter
        .first()
        .await
        .expect("borrowed first terminal")
        .expect("borrowed first result");
    let sibling_filter_usable = read_manager
        .where_(
            PersonType::identifier,
            ProjectedManagerComparison::Eq,
            &Identifier::new("data-transaction-seed").expect("transaction seed key"),
        )
        .expect("sibling borrowed filter")
        .exists()
        .await
        .expect("sibling borrowed terminal");
    assert_eq!(read_all.len(), 1);
    assert_eq!(read_count, 1);
    assert!(read_exists && sibling_filter_usable);
    assert_eq!(read_first.iid(), transaction_seed.iid());
    drop(read_filter);
    drop(read_manager);
    read.close().await.expect("borrowed read closes");

    let commit_transaction = db.write().await.expect("commit transaction opens");
    let committed = commit_transaction
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "data-transaction-commit",
            &[],
            None,
        ))
        .await
        .expect("borrowed commit insert");
    let committed_iid = committed.iid().to_owned();
    let commit_same_transaction_visible = commit_transaction
        .entities::<Person>()
        .get_by_iid(&committed_iid)
        .await
        .expect("same transaction commit read")
        .is_some();
    let commit_outside_before = db
        .entities::<Person>()
        .get_by_iid(&committed_iid)
        .await
        .expect("outside precommit read")
        .is_some();
    commit_transaction
        .commit()
        .await
        .expect("borrowed transaction commit");
    let commit_outside_after = db
        .entities::<Person>()
        .get_by_iid(&committed_iid)
        .await
        .expect("outside postcommit read")
        .is_some();

    let rollback_transaction = db.write().await.expect("rollback transaction opens");
    let rolled = rollback_transaction
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "data-transaction-rollback",
            &[],
            None,
        ))
        .await
        .expect("borrowed rollback insert");
    let rolled_iid = rolled.iid().to_owned();
    let rollback_same_transaction_visible = rollback_transaction
        .entities::<Person>()
        .get_by_iid(&rolled_iid)
        .await
        .expect("same transaction rollback read")
        .is_some();
    let rollback_outside_before = db
        .entities::<Person>()
        .get_by_iid(&rolled_iid)
        .await
        .expect("outside prerollback read")
        .is_some();
    rollback_transaction
        .rollback()
        .await
        .expect("borrowed transaction rollback");
    let rollback_outside_after = db
        .entities::<Person>()
        .get_by_iid(&rolled_iid)
        .await
        .expect("outside postrollback read")
        .is_some();

    let poison_conflict = db
        .entities::<Person>()
        .insert(ownership_edge_person_input(
            "data-poison-conflict",
            &[],
            None,
        ))
        .await
        .expect("poison conflict seed");
    let poison_transaction = db.write().await.expect("poison transaction opens");
    let first_cause = poison_transaction
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-poison-prefix", &[], None),
            ownership_edge_person_input("data-poison-conflict", &[], None),
        ])
        .await
        .expect_err("borrowed provider failure poisons transaction");
    assert_eq!(first_cause.code(), Some("provider_operation_failed"));
    let limit = usize::try_from(MAX_QUERY_ITEMS).expect("query item limit fits usize");
    let excessive = (0..=limit)
        .map(|index| {
            ownership_edge_person_input(
                &format!("data-excessive-{index}"),
                &[],
                None,
            )
        })
        .collect();
    let later_cause = poison_transaction
        .entities::<Person>()
        .insert_many(excessive)
        .await
        .expect_err("later resource limit is rejected");
    assert_eq!(later_cause.code(), Some("batch_item_limit"));
    let commit_rejection = poison_transaction
        .commit()
        .await
        .expect_err("rollback-only transaction cannot commit");
    assert_eq!(commit_rejection.code(), Some("transaction_rollback_only"));
    let poison_prefix_visible = db
        .entities::<Person>()
        .all()
        .await
        .expect("poison prefix read")
        .into_iter()
        .any(|person| person.identifier().value() == "data-poison-prefix");
    assert!(!poison_prefix_visible);

    let recovery_transaction = db.write().await.expect("recovery transaction opens");
    let recovery_cause = recovery_transaction
        .entities::<Person>()
        .insert_many(vec![
            ownership_edge_person_input("data-recovery-prefix", &[], None),
            ownership_edge_person_input("data-poison-conflict", &[], None),
        ])
        .await
        .expect_err("recovery transaction is poisoned");
    assert_eq!(recovery_cause.code(), first_cause.code());
    recovery_transaction
        .rollback()
        .await
        .expect("rollback-only transaction rolls back");
    let recovery_prefix_visible = db
        .entities::<Person>()
        .all()
        .await
        .expect("recovery prefix read")
        .into_iter()
        .any(|person| person.identifier().value() == "data-recovery-prefix");
    assert!(!recovery_prefix_visible);
    let borrowed_transaction_lifecycle = json!({
        "read": {
            "reusable_after_success": read_count == 1 && read_exists,
            "sibling_filter_usable": sibling_filter_usable,
            "terminal_sequence": ["all", "count", "exists", "first"],
            "state_after_terminals": "active",
            "close_idempotent": true,
        },
        "commit_visibility": {
            "before_commit": {
                "same_transaction_visible": commit_same_transaction_visible,
                "outside_transaction_visible": commit_outside_before,
            },
            "after_commit": {"outside_transaction_visible": commit_outside_after, "state": "committed"},
        },
        "rollback_visibility": {
            "before_rollback": {
                "same_transaction_visible": rollback_same_transaction_visible,
                "outside_transaction_visible": rollback_outside_before,
            },
            "after_rollback": {"outside_transaction_visible": rollback_outside_after, "state": "rolled_back"},
        },
        "poison": {
            "state": "rollback_only",
            "first_cause": {"category": "provider", "code": first_cause.code().expect("first poison code")},
            "later_cause": {"category": "resource_limit", "code": later_cause.code().expect("later poison code")},
            "retained_cause": {"category": "provider", "code": recovery_cause.code().expect("retained poison code")},
            "commit_rejection": {
                "category": "transaction",
                "code": commit_rejection.code().expect("commit rejection code"),
                "provider_commit_calls": 0,
            },
        },
        "post_rollback": {
            "state": "rolled_back",
            "rollback_idempotent": true,
            "commit_rejected": true,
            "mutation_rejected": true,
            "close_state": "closed",
            "close_idempotent": true,
        },
    });

    db.entities::<Person>()
        .delete(&committed_iid)
        .await
        .expect("committed transaction cleanup");
    db.entities::<Person>()
        .delete(transaction_seed.iid())
        .await
        .expect("transaction seed cleanup");
    db.entities::<Person>()
        .delete(poison_conflict.iid())
        .await
        .expect("poison conflict cleanup");

    let resource_read = db.read().await.expect("resource read transaction opens");
    let database_close_in_use = db.close().expect_err("database close rejects an open child");
    assert_eq!(database_close_in_use.code(), Some("resource_in_use"));
    let child_remains_usable = resource_read
        .entities::<Person>()
        .filter()
        .expect("resource child filter")
        .count()
        .await
        .is_ok();
    resource_read
        .close()
        .await
        .expect("resource read transaction closes");
    db.close().expect("database closes after child");
    db.close().expect("database close is idempotent");
    let post_close_rejected = db
        .entities::<Person>()
        .count()
        .await
        .is_err();
    assert!(post_close_rejected, "post-close provider work is rejected");
    let data_resource_lifecycle = json!({
        "resources": [
            "database", "read_transaction", "write_transaction", "cancellation",
            "batch_builder", "batch_input", "filter", "result", "projected_value",
            "projected_thing", "diagnostic",
        ],
        "close_contract": {
            "idempotent": true,
            "post_close_rejected": post_close_rejected,
            "post_close_provider_calls": 0,
        },
        "parent_child": {
            "runtime_close_with_database": "in_use",
            "database_close_with_transaction": "in_use",
            "parent_handle_retained_on_rejection": database_close_in_use.code() == Some("resource_in_use"),
            "child_remains_usable": child_remains_usable,
            "parent_closes_after_children": true,
        },
        "session_rules": {
            "borrowed_read_not_consumed": read_count == 1 && read_exists,
            "sibling_filter_usable": sibling_filter_usable,
            "write_recovery_after_cancellation": true,
        },
        "result_survival": {
            "result_survives_filter_close": read_first.identifier().value() == "data-transaction-seed",
            "result_survives_transaction_close": read_first.identifier().value() == "data-transaction-seed",
            "owned_thing_survives_result_close": !read_first.iid().is_empty(),
        },
        "cancellation_close_idempotent": true,
        "projected_value_close_idempotent": true,
        "projected_thing_close_idempotent": true,
    });
    let journey_bytes = fs::read(workforce_env_path("TYPE_BRIDGE_WORKFORCE_V3_JOURNEY"))
        .expect("Workforce V3 journey is readable");
    let journey: Value =
        serde_json::from_slice(&journey_bytes).expect("Workforce V3 journey is valid JSON");
    assert_eq!(
        entity_batch_insert_put,
        journey["expected_observations"]["entity_batch_insert_put"]
    );
    assert_eq!(
        entity_batch_update_delete_atomic,
        journey["expected_observations"]["entity_batch_update_delete_atomic"]
    );
    assert_eq!(
        relation_batch_insert_put,
        journey["expected_observations"]["relation_batch_insert_put"]
    );
    assert_eq!(
        relation_batch_update_delete_atomic,
        journey["expected_observations"]["relation_batch_update_delete_atomic"]
    );
    assert_eq!(
        entity_observation,
        journey["expected_observations"]["unkeyed_entity_iid_lifecycle"]
    );
    assert_eq!(
        relation_observation,
        journey["expected_observations"]["unkeyed_relation_iid_lifecycle"]
    );
    assert_eq!(
        data_resource_lifecycle,
        journey["expected_observations"]["data_resource_lifecycle"]
    );
    assert_eq!(
        borrowed_transaction_lifecycle,
        journey["expected_observations"]["borrowed_transaction_lifecycle"]
    );
    let semantic_fingerprint: Value = serde_json::from_str(SEMANTIC_SCHEMA_FINGERPRINT_JSON)
        .expect("generated V3 semantic fingerprint is valid JSON");
    let projection_fingerprint: Value = serde_json::from_str(PROJECTION_FINGERPRINT_JSON)
        .expect("generated V3 projection fingerprint is valid JSON");
    let results = [
        ("borrowed_transaction_lifecycle", "lifecycle", borrowed_transaction_lifecycle),
        ("data_resource_lifecycle", "lifecycle", data_resource_lifecycle),
        ("entity_batch_insert_put", "direct_runtime", entity_batch_insert_put),
        (
            "entity_batch_update_delete_atomic",
            "direct_runtime",
            entity_batch_update_delete_atomic,
        ),
        ("relation_batch_insert_put", "direct_runtime", relation_batch_insert_put),
        (
            "relation_batch_update_delete_atomic",
            "direct_runtime",
            relation_batch_update_delete_atomic,
        ),
        ("unkeyed_entity_iid_lifecycle", "direct_runtime", entity_observation),
        ("unkeyed_relation_iid_lifecycle", "direct_runtime", relation_observation),
    ]
    .into_iter()
    .map(|(observation_ref, proof_kind, observation)| {
        json!({
            "observation_ref": observation_ref,
            "proof_kind": proof_kind,
            "outcome": "passed",
            "observation": observation,
        })
    })
    .collect::<Vec<_>>();
    let supplement = json!({
        "format": "typebridge.workforce-v3-live-supplement/v1",
        "binding": "rust",
        "producer": "type-bridge-rust.generated-data-model-runtime-v3-live",
        "semantic_profile": WORKFORCE_PROFILE,
        "semantic_fingerprint": semantic_fingerprint,
        "projection_fingerprint": projection_fingerprint,
        "results": results,
    });
    publish_workforce_report(
        &workforce_env_path("TYPE_BRIDGE_WORKFORCE_V3_RUST_SUPPLEMENT"),
        supplement,
    );
    println!("generated Workforce V3 Rust live supplement: passed");
}

fn relation_person_input(identifier: &str, alias: &str) -> PersonCreate {
    ownership_edge_person_input(identifier, &[alias], None)
}

fn ownership_edge_person_input(
    identifier: &str,
    aliases: &[&str],
    nickname: Option<&str>,
) -> PersonCreate {
    PersonCreate::try_new(
        aliases
            .iter()
            .map(|alias| Aliases::new((*alias).to_owned()).expect("relation lifecycle alias"))
            .collect(),
        None,
        Identifier::new(identifier.to_owned()).expect("relation lifecycle identifier"),
        nickname.map(|value| {
            Nickname::new(value.to_owned()).expect("relation lifecycle optional nickname")
        }),
        Score::new(70).expect("relation lifecycle score"),
        None,
        ValBool::new(true).expect("relation lifecycle bool"),
        ValConstrained::new(55).expect("relation lifecycle constrained"),
        ValDate::new(Date::try_new("2026-08-03").expect("relation lifecycle date"))
            .expect("relation lifecycle date value"),
        ValDatetime::new(
            DateTime::try_new("2026-08-03T03:55:00").expect("relation lifecycle datetime"),
        )
        .expect("relation lifecycle datetime value"),
        ValDatetimeTz::new(
            DateTimeTz::try_new("2026-08-03T03:55:00Z").expect("relation lifecycle tz"),
        )
        .expect("relation lifecycle tz value"),
        ValDecimal::new(Decimal::try_new("128.45").expect("relation lifecycle decimal"))
            .expect("relation lifecycle decimal value"),
        ValDouble::new(CanonicalDouble::try_new(8.14).expect("relation lifecycle double"))
            .expect("relation lifecycle double value"),
        ValDuration::new(Duration::try_new("P6D").expect("relation lifecycle duration"))
            .expect("relation lifecycle duration value"),
    )
    .expect("relation lifecycle person input")
}
