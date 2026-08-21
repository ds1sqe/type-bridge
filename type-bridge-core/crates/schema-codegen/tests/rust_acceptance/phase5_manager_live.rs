use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use generated::*;
use generated_foreign as foreign;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use type_bridge::__codegen::FieldToken;
use type_bridge::{ConnectionOptions, Database as GeneratedDatabase, ProjectedManagerComparison};
use type_bridge_orm::session::{ConnectOptions, Database as AdminDatabase, TxType};

const REPORT_FORMAT: &str = "typebridge.phase5-manager-filter-live-report/v1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const SERVER_VERSION: &str = "3.12.3";
const OUTPUT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_REPOSITORY_ROOT";
const USERNAME: &str = "admin";
const PASSWORD: &str = "password";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const EXPECTED_SEMANTIC_FINGERPRINT: &str = "{\"algorithm\":\"sha256\",\"canonicalization\":\"typebridge.schema-canonical-json/v1\",\"digest\":\"3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8\",\"domain\":\"typebridge.schema.semantic\",\"semantic_profile\":\"typedb-3.12.1/v1\"}";

const AUTHORITY_FILES: [(&str, &str, &str); 3] = [
    (
        "schema",
        "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
        "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    ),
    (
        "journey",
        "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
        "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
    ),
    (
        "provider",
        "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
        "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
    ),
];

#[derive(Debug)]
struct ProducerError {
    code: &'static str,
    message: String,
}

impl ProducerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn caused(code: &'static str, message: impl Into<String>, cause: impl fmt::Display) -> Self {
        Self::new(code, format!("{}: {cause}", message.into()))
    }

    fn with_cleanup(mut self, cleanup: &Self) -> Self {
        self.message = format!(
            "{}; cleanup also failed with {}: {}",
            self.message, cleanup.code, cleanup.message,
        );
        self
    }
}

impl fmt::Display for ProducerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProducerError {}

type ProducerResult<T> = Result<T, ProducerError>;

struct Config {
    output: PathBuf,
    address: String,
    database: String,
    http_port: u16,
    repository: PathBuf,
}

struct Authority {
    report: Value,
    journey: Value,
    provider: String,
}

#[derive(Clone)]
struct PersonInput {
    identifier: String,
    nickname: Option<String>,
    score: i64,
    foo_bar: i64,
    score_gte: i64,
    val_bool: bool,
    val_constrained: i64,
    val_date: String,
    val_datetime: String,
    val_datetime_tz: String,
    val_decimal: String,
    val_double_bits: u64,
    val_duration: String,
}

fn required_environment(name: &'static str) -> ProducerResult<String> {
    let value = env::var(name)
        .map_err(|error| ProducerError::caused("missing_environment", name, error))?;
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(ProducerError::new(
            "invalid_environment",
            format!("{name} must be bounded non-empty text"),
        ));
    }
    Ok(value)
}

fn parse_config() -> ProducerResult<Config> {
    let output = PathBuf::from(required_environment(OUTPUT_ENV)?);
    if !output.is_absolute() {
        return Err(ProducerError::new(
            "invalid_output_path",
            format!("{OUTPUT_ENV} must be absolute"),
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| ProducerError::new("invalid_output_path", "report has no parent"))?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        ProducerError::caused(
            "invalid_output_path",
            "report parent is not inspectable",
            error,
        )
    })?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(ProducerError::new(
            "invalid_output_path",
            "report parent must be a real directory",
        ));
    }
    match fs::symlink_metadata(&output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ProducerError::caused(
                "invalid_output_path",
                "report destination is not inspectable",
                error,
            ));
        }
        Ok(_) => return Err(ProducerError::new("output_exists", "report already exists")),
    }

    let address = required_environment(ADDRESS_ENV)?;
    if address.contains("://") || address.contains('@') || address.chars().any(char::is_whitespace)
    {
        return Err(ProducerError::new(
            "invalid_address",
            format!("{ADDRESS_ENV} must be a credential-free host:port"),
        ));
    }
    let database = required_environment(DATABASE_ENV)?;
    if database.len() > 256
        || !database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ProducerError::new(
            "invalid_database",
            format!("{DATABASE_ENV} is not a bounded ASCII database name"),
        ));
    }
    let port = required_environment(HTTP_PORT_ENV)?;
    if !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProducerError::new(
            "invalid_http_port",
            format!("{HTTP_PORT_ENV} must be an ASCII integer"),
        ));
    }
    let http_port = port.parse::<u16>().map_err(|error| {
        ProducerError::caused(
            "invalid_http_port",
            format!("{HTTP_PORT_ENV} must be in 1..65535"),
            error,
        )
    })?;
    if http_port == 0 {
        return Err(ProducerError::new(
            "invalid_http_port",
            format!("{HTTP_PORT_ENV} must be in 1..65535"),
        ));
    }
    let root = PathBuf::from(required_environment(ROOT_ENV)?);
    if !root.is_absolute() {
        return Err(ProducerError::new(
            "invalid_repository_root",
            format!("{ROOT_ENV} must be absolute"),
        ));
    }
    let repository = root.canonicalize().map_err(|error| {
        ProducerError::caused(
            "invalid_repository_root",
            "repository root cannot be canonicalized",
            error,
        )
    })?;
    Ok(Config {
        output,
        address,
        database,
        http_port,
        repository,
    })
}

fn bounded_regular_bytes(path: &Path, label: &str) -> ProducerResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProducerError::caused(
            "invalid_authority",
            format!("{label} is not inspectable"),
            error,
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ProducerError::new(
            "invalid_authority",
            format!("{label} must be a regular non-symlink file"),
        ));
    }
    if metadata.len() > MAX_AUTHORITY_BYTES {
        return Err(ProducerError::new(
            "authority_size_limit",
            format!("{label} is too large"),
        ));
    }
    fs::read(path).map_err(|error| {
        ProducerError::caused(
            "invalid_authority",
            format!("{label} cannot be read"),
            error,
        )
    })
}

fn load_authority(repository: &Path) -> ProducerResult<Authority> {
    let mut report = serde_json::Map::new();
    let mut journey = None;
    let mut provider = None;
    for (label, relative, expected) in AUTHORITY_FILES {
        let bytes = bounded_regular_bytes(&repository.join(relative), label)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != expected {
            return Err(ProducerError::new(
                "authority_hash_mismatch",
                format!("Phase-5 {label} authority drifted"),
            ));
        }
        report.insert(
            label.to_owned(),
            json!({"path": relative, "sha256": digest}),
        );
        if label == "journey" {
            journey = Some(serde_json::from_slice(&bytes).map_err(|error| {
                ProducerError::caused("invalid_journey", "journey is not JSON", error)
            })?);
        } else if label == "provider" {
            provider = Some(String::from_utf8(bytes).map_err(|error| {
                ProducerError::caused("invalid_provider", "provider schema is not UTF-8", error)
            })?);
        }
    }
    Ok(Authority {
        report: Value::Object(report),
        journey: journey.expect("journey is in fixed authority list"),
        provider: provider.expect("provider is in fixed authority list"),
    })
}

fn generated<T, E: fmt::Display>(result: Result<T, E>, context: &'static str) -> ProducerResult<T> {
    result.map_err(|error| ProducerError::caused("generated_value_rejected", context, error))
}

fn live<T, E: fmt::Display>(result: Result<T, E>, context: &'static str) -> ProducerResult<T> {
    result.map_err(|error| ProducerError::caused("live_operation_failed", context, error))
}

fn record<'a>(journey: &'a Value, reference: &str) -> ProducerResult<&'a Value> {
    journey["records"]["people"]
        .as_array()
        .and_then(|people| people.iter().find(|person| person["ref"] == reference))
        .ok_or_else(|| ProducerError::new("invalid_journey", format!("missing person {reference}")))
}

fn field<'a>(record: &'a Value, name: &str, kind: &str) -> ProducerResult<&'a Value> {
    let value = &record["fields"][name];
    if value["kind"] != kind {
        return Err(ProducerError::new(
            "invalid_journey",
            format!("{name} is not {kind}"),
        ));
    }
    Ok(value)
}

fn string_field(record: &Value, name: &str, kind: &str) -> ProducerResult<String> {
    field(record, name, kind)?["value"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ProducerError::new("invalid_journey", format!("{name} is not text")))
}

fn long_field(record: &Value, name: &str) -> ProducerResult<i64> {
    string_field(record, name, "long")?
        .parse()
        .map_err(|error| ProducerError::caused("invalid_journey", name, error))
}

fn person_input(journey: &Value, reference: &str) -> ProducerResult<PersonInput> {
    let value = record(journey, reference)?;
    let nickname = value["fields"]
        .get("nickname")
        .map(|_| string_field(value, "nickname", "string"))
        .transpose()?;
    let bits = field(value, "val_double", "double")?["bits"]
        .as_str()
        .ok_or_else(|| ProducerError::new("invalid_journey", "double bits are not text"))?;
    Ok(PersonInput {
        identifier: string_field(value, "identifier", "string")?,
        nickname,
        score: long_field(value, "score")?,
        foo_bar: long_field(value, "foo__bar")?,
        score_gte: long_field(value, "score__gte")?,
        val_bool: field(value, "val_bool", "boolean")?["value"]
            .as_bool()
            .ok_or_else(|| ProducerError::new("invalid_journey", "val_bool is not boolean"))?,
        val_constrained: long_field(value, "val_constrained")?,
        val_date: string_field(value, "val_date", "date")?,
        val_datetime: string_field(value, "val_datetime", "datetime")?,
        val_datetime_tz: string_field(value, "val_datetime_tz", "datetime_tz")?,
        val_decimal: string_field(value, "val_decimal", "decimal")?,
        val_double_bits: u64::from_str_radix(bits, 16).map_err(|error| {
            ProducerError::caused("invalid_journey", "double bits are malformed", error)
        })?,
        val_duration: string_field(value, "val_duration", "duration")?,
    })
}

fn person_create(input: &PersonInput) -> ProducerResult<PersonCreate> {
    generated(
        PersonCreate::new(
            Vec::new(),
            Some(generated(FooBar::new(input.foo_bar), "person foo__bar")?),
            generated(
                Identifier::new(input.identifier.clone()),
                "person identifier",
            )?,
            input
                .nickname
                .as_ref()
                .map(|value| generated(Nickname::new(value.clone()), "person nickname"))
                .transpose()?,
            generated(Score::new(input.score), "person score")?,
            Some(generated(
                ScoreGte::new(input.score_gte),
                "person score__gte",
            )?),
            generated(ValBool::new(input.val_bool), "person val_bool")?,
            generated(
                ValConstrained::new(input.val_constrained),
                "person val_constrained",
            )?,
            generated(
                ValDate::new(generated(Date::try_new(&input.val_date), "person date")?),
                "person val_date",
            )?,
            generated(
                ValDatetime::new(generated(
                    DateTime::try_new(&input.val_datetime),
                    "person datetime",
                )?),
                "person val_datetime",
            )?,
            generated(
                ValDatetimeTz::new(generated(
                    DateTimeTz::try_new(&input.val_datetime_tz),
                    "person datetime_tz",
                )?),
                "person val_datetime_tz",
            )?,
            generated(
                ValDecimal::new(generated(
                    Decimal::try_new(&input.val_decimal),
                    "person decimal",
                )?),
                "person val_decimal",
            )?,
            generated(
                ValDouble::new(generated(
                    CanonicalDouble::try_from_bits(input.val_double_bits),
                    "person double",
                )?),
                "person val_double",
            )?,
            generated(
                ValDuration::new(generated(
                    Duration::try_new(&input.val_duration),
                    "person duration",
                )?),
                "person val_duration",
            )?,
        ),
        "person create",
    )
}

fn person_keys(values: &[Person]) -> ProducerResult<Vec<String>> {
    let mut keys = values
        .iter()
        .map(|person| person.identifier().value().clone())
        .collect::<Vec<_>>();
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ProducerError::new(
            "duplicate_identity",
            "manager all returned a duplicate identity",
        ));
    }
    Ok(keys)
}

fn require_diagnostic(
    error: type_bridge::Error,
    kind: &'static str,
    category: &'static str,
    code: &'static str,
) -> ProducerResult<Value> {
    if error.sdk_category() != Some(category) || error.code() != Some(code) {
        return Err(ProducerError::new(
            "diagnostic_mismatch",
            format!(
                "{kind} returned {:?}/{:?}",
                error.sdk_category(),
                error.code(),
            ),
        ));
    }
    Ok(json!({
        "category": category,
        "code": code,
        "kind": kind,
        "rejected_before_provider_io": true,
    }))
}

fn require_where_error<T>(
    result: type_bridge::Result<T>,
    kind: &'static str,
    category: &'static str,
    code: &'static str,
) -> ProducerResult<Value> {
    match result {
        Ok(_) => Err(ProducerError::new(
            "missing_rejection",
            format!("{kind} unexpectedly succeeded"),
        )),
        Err(error) => require_diagnostic(error, kind, category, code),
    }
}

async fn pre_io_observations(
    database: &GeneratedDatabase<AppSchema>,
) -> ProducerResult<(Vec<Value>, Value)> {
    let wrong_owner = FieldToken::<Person, RobotId>::new(
        RobotType::robot_id.owns_id_json(),
        RobotType::robot_id.metadata_json(),
    );
    let wrong_domain = FieldToken::<Person, Identifier>::new(
        PersonType::foo__bar.owns_id_json(),
        PersonType::foo__bar.metadata_json(),
    );
    let wrong_package = FieldToken::<Person, FooBar>::new(
        foreign::PersonType::foo__bar.owns_id_json(),
        foreign::PersonType::foo__bar.metadata_json(),
    );
    let rejections = vec![
        require_where_error(
            database.entities::<Person>().where_(
                wrong_owner,
                ProjectedManagerComparison::Eq,
                &generated(RobotId::new(7), "wrong-owner literal")?,
            ),
            "wrong_field_owner",
            "integrity",
            "field_owner_mismatch",
        )?,
        require_where_error(
            database.entities::<Person>().where_(
                wrong_domain,
                ProjectedManagerComparison::Eq,
                &generated(Identifier::new("wrong-domain"), "wrong-domain literal")?,
            ),
            "wrong_scalar_domain",
            "invalid_input",
            "wrong_scalar_domain",
        )?,
        require_where_error(
            database.entities::<Person>().where_(
                wrong_package,
                ProjectedManagerComparison::Eq,
                &generated(FooBar::new(7), "wrong-package literal")?,
            ),
            "wrong_package",
            "integrity",
            "generated_token_package_mismatch",
        )?,
        require_where_error(
            database.entities::<Person>().where_(
                PersonType::val_bool,
                ProjectedManagerComparison::Gt,
                &generated(ValBool::new(true), "boolean-ordering literal")?,
            ),
            "boolean_ordering",
            "invalid_input",
            "invalid_operator_for_type",
        )?,
    ];

    let nonsingular = database.entities::<Person>().where_(
        PersonType::foo__bar,
        ProjectedManagerComparison::Gte,
        &generated(FooBar::new(7), "nonsingular literal")?,
    );
    let nonsingular = live(nonsingular, "compose nonsingular first filter")?;
    let error = match nonsingular.first().await {
        Ok(_) => {
            return Err(ProducerError::new(
                "missing_rejection",
                "nonsingular first unexpectedly succeeded",
            ));
        }
        Err(error) => error,
    };
    let first = require_diagnostic(
        error,
        "nonsingular_first",
        "invalid_input",
        "manager_first_requires_identity",
    )?;
    Ok((
        rejections,
        json!({
            "category": first["category"],
            "code": first["code"],
            "rejected_before_provider_io": first["rejected_before_provider_io"],
        }),
    ))
}

fn field_token_observation() -> ProducerResult<Value> {
    let owns: Value =
        serde_json::from_str(PersonType::foo__bar.owns_id_json()).map_err(|error| {
            ProducerError::caused("generated_token_identity", "owns ID is malformed", error)
        })?;
    let metadata: Value =
        serde_json::from_str(PersonType::foo__bar.metadata_json()).map_err(|error| {
            ProducerError::caused("generated_token_identity", "metadata is malformed", error)
        })?;
    if owns != json!({"attribute": "foo__bar", "owner": {"kind": "entity", "label": "person"}})
        || metadata["id"] != owns
        || metadata["target_name"] != "foo__bar"
    {
        return Err(ProducerError::new(
            "generated_token_identity",
            "generated foo__bar token identity drifted",
        ));
    }
    Ok(json!({
        "attribute": "attribute:foo__bar",
        "binding_name": "foo__bar",
        "owner": "entity:person",
    }))
}

async fn manager_observation(database: &GeneratedDatabase<AppSchema>) -> ProducerResult<Value> {
    let (rejections, nonsingular_rejection) = pre_io_observations(database).await?;
    let read = live(database.read().await, "open reusable borrowed read")?;
    let journey = async {
        let manager = read.entities::<Person>();
        let root = live(manager.filter(), "compose root manager filter")?;
        let seven = generated(FooBar::new(7), "operator literal")?;
        let mut outcomes = Vec::new();
        for (name, comparison) in [
            ("eq", ProjectedManagerComparison::Eq),
            ("ne", ProjectedManagerComparison::Ne),
            ("gt", ProjectedManagerComparison::Gt),
            ("gte", ProjectedManagerComparison::Gte),
            ("lt", ProjectedManagerComparison::Lt),
            ("lte", ProjectedManagerComparison::Lte),
        ] {
            let selected = live(
                live(
                    manager.where_(PersonType::foo__bar, comparison, &seven),
                    "compose operator filter",
                )?
                .all()
                .await,
                "execute operator filter",
            )?;
            outcomes.push(json!({
                "normalized_keys": person_keys(&selected)?,
                "operator": name,
            }));
        }

        let forty = generated(Score::new(40), "conjunction score")?;
        let conjunction = live(
            manager.where_(
                PersonType::foo__bar,
                ProjectedManagerComparison::Gte,
                &seven,
            ),
            "compose conjunction first predicate",
        )?;
        let conjunction = live(
            conjunction.where_(PersonType::score, ProjectedManagerComparison::Gt, &forty),
            "compose conjunction second predicate",
        )?;
        let conjunction_keys = person_keys(&live(conjunction.all().await, "execute conjunction")?)?;

        let all_keys = person_keys(&live(root.all().await, "root all")?)?;
        let count = live(root.count().await, "root count")?;
        let exists = live(root.exists().await, "root exists")?;
        if all_keys != ["data-ada", "data-dana"] || count != 2 || !exists {
            return Err(ProducerError::new(
                "terminal_mismatch",
                "root manager terminals drifted",
            ));
        }

        let ada_key = generated(Identifier::new("data-ada"), "identity literal")?;
        let first = live(
            live(
                manager.where_(
                    PersonType::identifier,
                    ProjectedManagerComparison::Eq,
                    &ada_key,
                ),
                "compose identity filter",
            )?
            .first()
            .await,
            "execute strict first",
        )?
        .ok_or_else(|| ProducerError::new("terminal_mismatch", "strict first returned none"))?;
        if first.identifier().value() != "data-ada" {
            return Err(ProducerError::new(
                "terminal_mismatch",
                "strict first returned the wrong person",
            ));
        }

        let mut session = read.query();
        let person = live(session.exact::<Person>(), "bind Plan04 person")?;
        let plan04 = live(
            live(session.query(person), "select Plan04 person")?.where_(
                person
                    .field(PersonType::identifier)
                    .eq(generated(Identifier::new("data-ada"), "Plan04 identity")?),
            ),
            "filter Plan04 person",
        )?;
        if live(plan04.count_by(person).await, "execute Plan04 count")? != 1 {
            return Err(ProducerError::new(
                "borrowed_read_reuse",
                "Plan04 query did not remain usable",
            ));
        }
        drop(plan04);
        drop(session);

        let nine = generated(FooBar::new(9), "sibling literal")?;
        let sibling = live(
            manager.where_(PersonType::foo__bar, ProjectedManagerComparison::Eq, &nine),
            "compose sibling manager filter",
        )?;
        if person_keys(&live(
            sibling.all().await,
            "execute sibling manager filter",
        )?)? != ["data-dana"]
        {
            return Err(ProducerError::new(
                "borrowed_read_reuse",
                "sibling manager filter did not remain usable",
            ));
        }

        Ok(json!({
            "borrowed_read": {
                "final_state": "active",
                "reusable_after_each_terminal": true,
                "sibling_filter_usable": true,
            },
            "conjunction": {
                "authored_order": ["foo__bar:gte:7", "score:gt:40"],
                "normalized_keys": conjunction_keys,
            },
            "field_token": field_token_observation()?,
            "first": {
                "identity_predicate": "identifier:eq:data-ada",
                "nonsingular_rejection": nonsingular_rejection,
                "result": first.identifier().value(),
                "strict_singular": true,
            },
            "model": "person",
            "operator_literal": {"kind": "long", "value": "7"},
            "operator_outcomes": outcomes,
            "rejections": rejections,
            "terminals": {
                "all_normalization": "reference_key_ascending",
                "count": count,
                "exists": exists,
            },
        }))
    }
    .await;
    let close = live(read.close().await, "close reusable borrowed read");
    match (journey, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(error.with_cleanup(&cleanup)),
    }
}

async fn run_database(config: &Config, authority: &Authority) -> ProducerResult<Value> {
    let admin = live(
        AdminDatabase::connect_with_options(
            &config.address,
            &config.database,
            USERNAME,
            PASSWORD,
            ConnectOptions {
                http_port: config.http_port,
                tls: false,
                ..ConnectOptions::default()
            },
        )
        .await,
        "connect administrative handle",
    )?;
    let mut owns_database = false;
    let primary = async {
        let detected = admin.server_version().ok_or_else(|| {
            ProducerError::new("missing_server_version", "provider version is absent")
        })?;
        if detected.to_string() != SERVER_VERSION {
            return Err(ProducerError::new(
                "server_version_mismatch",
                format!("expected {SERVER_VERSION}, detected {detected}"),
            ));
        }
        if live(admin.database_exists().await, "check database absence")? {
            return Err(ProducerError::new(
                "database_preexisting",
                "isolated database must initially be absent",
            ));
        }
        live(admin.create_database().await, "create isolated database")?;
        owns_database = true;
        live(
            admin.execute_raw(&authority.provider, TxType::Schema).await,
            "install frozen provider schema",
        )?;
        let unbound = live(
            GeneratedDatabase::connect(
                ConnectionOptions::new(&config.address, &config.database)
                    .credentials(USERNAME, PASSWORD)
                    .http_port(config.http_port)
                    .tls(false),
            )
            .await,
            "connect generated database",
        )?;
        let database = live(unbound.with_schema(SCHEMA), "bind generated package")?;
        let ada = person_input(&authority.journey, "data-ada")?;
        let dana = person_input(&authority.journey, "data-dana")?;
        let ada = live(
            database
                .entities::<Person>()
                .insert(person_create(&ada)?)
                .await,
            "insert Ada",
        )?;
        let dana = live(
            database
                .entities::<Person>()
                .insert(person_create(&dana)?)
                .await,
            "insert Dana",
        )?;
        let observation = manager_observation(&database).await?;
        live(
            database.entities::<Person>().delete(dana.iid()).await,
            "delete Dana",
        )?;
        live(
            database.entities::<Person>().delete(ada.iid()).await,
            "delete Ada",
        )?;
        if live(
            database.entities::<Person>().count().await,
            "final person count",
        )? != 0
        {
            return Err(ProducerError::new(
                "cleanup_failed",
                "person rows remained after cleanup",
            ));
        }
        drop(database);
        Ok(observation)
    }
    .await;

    let delete = if owns_database {
        live(admin.delete_database().await, "delete isolated database")
    } else {
        Ok(())
    };
    let close = live(admin.close(), "close administrative handle");
    match (primary, delete, close) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (Err(error), Ok(()), Ok(())) => Err(error),
        (Err(error), Err(cleanup), Ok(())) | (Err(error), Ok(()), Err(cleanup)) => {
            Err(error.with_cleanup(&cleanup))
        }
        (Err(error), Err(delete), Err(close)) => {
            Err(error.with_cleanup(&delete).with_cleanup(&close))
        }
        (Ok(_), Err(error), Ok(())) | (Ok(_), Ok(()), Err(error)) => Err(error),
        (Ok(_), Err(delete), Err(close)) => Err(delete.with_cleanup(&close)),
    }
}

fn sorted_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sorted_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, sorted_json(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        scalar => scalar,
    }
}

fn publish(path: &Path, report: Value) -> ProducerResult<()> {
    let mut bytes = serde_json::to_vec(&sorted_json(report)).map_err(|error| {
        ProducerError::caused("report_serialization_failed", "report is not JSON", error)
    })?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(ProducerError::new(
            "report_size_limit",
            "report is too large",
        ));
    }
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            ProducerError::caused(
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "output_exists"
                } else {
                    "report_publication_failed"
                },
                "report destination cannot be created",
                error,
            )
        })?;
    if let Err(error) = destination
        .write_all(&bytes)
        .and_then(|()| destination.sync_all())
    {
        drop(destination);
        let _ = fs::remove_file(path);
        return Err(ProducerError::caused(
            "report_publication_failed",
            "report could not be written completely",
            error,
        ));
    }
    Ok(())
}

async fn run() -> ProducerResult<()> {
    if SEMANTIC_SCHEMA_FINGERPRINT_JSON != EXPECTED_SEMANTIC_FINGERPRINT {
        return Err(ProducerError::new(
            "generated_package_mismatch",
            "generated crate is not the exact Workforce V3 projection",
        ));
    }
    if foreign::SEMANTIC_SCHEMA_FINGERPRINT_JSON == SEMANTIC_SCHEMA_FINGERPRINT_JSON
        || foreign::PersonType::foo__bar.metadata_json() == PersonType::foo__bar.metadata_json()
    {
        return Err(ProducerError::new(
            "foreign_package_mismatch",
            "foreign foo__bar package is not genuinely different",
        ));
    }
    let config = parse_config()?;
    let authority = load_authority(&config.repository)?;
    let observation = run_database(&config, &authority).await?;
    publish(
        &config.output,
        json!({
            "authority": authority.report,
            "binding": "rust",
            "format": REPORT_FORMAT,
            "observation": observation,
            "semantic_profile": SEMANTIC_PROFILE,
        }),
    )
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Phase-5 Rust manager live producer rejected: {error}");
        std::process::exit(1);
    }
}
