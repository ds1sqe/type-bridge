use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use generated::*;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use type_bridge::{ConnectionOptions, Database as GeneratedDatabase};
use type_bridge_orm::session::{ConnectOptions, Database as AdminDatabase, TxType};

const REPORT_FORMAT: &str = "typebridge.phase2-projected-live-report/v1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const SERVER_VERSION: &str = "3.12.3";
const OUTPUT_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT";
const USERNAME: &str = "admin";
const PASSWORD: &str = "password";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_ENDPOINT_BYTES: usize = 4096;
const MAX_DATABASE_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4096;
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

    fn with_cleanup(mut self, cleanup: &ProducerError) -> Self {
        self.message = format!(
            "{}; cleanup also failed with {}: {}",
            self.message, cleanup.code, cleanup.message
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
    provider_typeql: String,
}

struct Created {
    ada: String,
    dana: String,
    positive_robot: String,
    negative_robot: String,
    interaction_robot: String,
    interaction_absent: String,
    interaction_person: String,
    plain_activity: String,
    event: String,
    container: String,
}

struct PersonFixture<'a> {
    identifier: &'a str,
    nickname: Option<&'a str>,
    score: i64,
    foo_bar: i64,
    score_gte: i64,
    val_bool: bool,
    date: &'a str,
    datetime: &'a str,
    datetime_tz: &'a str,
    decimal: &'a str,
    double_bits: u64,
    duration: &'a str,
}

fn required_environment(name: &'static str) -> ProducerResult<String> {
    let value = env::var(name)
        .map_err(|error| ProducerError::caused("missing_environment", name, error))?;
    if value.is_empty() {
        return Err(ProducerError::new(
            "missing_environment",
            format!("{name} must not be empty"),
        ));
    }
    Ok(value)
}

fn parse_config() -> ProducerResult<Config> {
    let output_value = required_environment(OUTPUT_ENV)?;
    if output_value.len() > MAX_PATH_BYTES {
        return Err(ProducerError::new(
            "path_size_limit",
            format!("{OUTPUT_ENV} exceeds {MAX_PATH_BYTES} bytes"),
        ));
    }
    let output = PathBuf::from(output_value);
    if !output.is_absolute() {
        return Err(ProducerError::new(
            "invalid_output_path",
            format!("{OUTPUT_ENV} must be absolute"),
        ));
    }
    match fs::symlink_metadata(&output) {
        Ok(_) => {
            return Err(ProducerError::new(
                "output_exists",
                format!("{} already exists", output.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ProducerError::caused(
                "invalid_output_path",
                format!("{} cannot be inspected", output.display()),
                error,
            ));
        }
    }
    let parent = output.parent().ok_or_else(|| {
        ProducerError::new("invalid_output_path", "report path has no parent directory")
    })?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        ProducerError::caused(
            "invalid_output_path",
            format!("report parent {} cannot be inspected", parent.display()),
            error,
        )
    })?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(ProducerError::new(
            "invalid_output_path",
            "report parent must be a real directory",
        ));
    }

    let address = required_environment(ADDRESS_ENV)?;
    if address.len() > MAX_ENDPOINT_BYTES
        || address.trim() != address
        || address.chars().any(char::is_whitespace)
        || address.contains("://")
        || address.contains('@')
    {
        return Err(ProducerError::new(
            "invalid_address",
            format!("{ADDRESS_ENV} must be one bounded credential-free host:port"),
        ));
    }

    let database = required_environment(DATABASE_ENV)?;
    if database.len() > MAX_DATABASE_BYTES
        || !database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ProducerError::new(
            "invalid_database",
            format!("{DATABASE_ENV} must be a bounded ASCII database name"),
        ));
    }

    let port_value = required_environment(HTTP_PORT_ENV)?;
    if !port_value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProducerError::new(
            "invalid_http_port",
            format!("{HTTP_PORT_ENV} must be an ASCII integer"),
        ));
    }
    let http_port = port_value.parse::<u16>().map_err(|error| {
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

    let repository_value = required_environment(ROOT_ENV)?;
    if repository_value.len() > MAX_PATH_BYTES {
        return Err(ProducerError::new(
            "path_size_limit",
            format!("{ROOT_ENV} exceeds {MAX_PATH_BYTES} bytes"),
        ));
    }
    let repository_input = PathBuf::from(repository_value);
    if !repository_input.is_absolute() {
        return Err(ProducerError::new(
            "invalid_repository_root",
            format!("{ROOT_ENV} must be absolute"),
        ));
    }
    let repository = repository_input.canonicalize().map_err(|error| {
        ProducerError::caused(
            "invalid_repository_root",
            format!("{} cannot be canonicalized", repository_input.display()),
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
            "invalid_authority_file",
            format!("{label} cannot be inspected"),
            error,
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ProducerError::new(
            "invalid_authority_file",
            format!("{label} must be a regular non-symlink file"),
        ));
    }
    if metadata.len() > MAX_AUTHORITY_BYTES {
        return Err(ProducerError::new(
            "authority_size_limit",
            format!("{label} exceeds {MAX_AUTHORITY_BYTES} bytes"),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        ProducerError::caused(
            "invalid_authority_file",
            format!("{label} cannot be read"),
            error,
        )
    })?;
    if bytes.len() as u64 > MAX_AUTHORITY_BYTES {
        return Err(ProducerError::new(
            "authority_size_limit",
            format!("{label} exceeds {MAX_AUTHORITY_BYTES} bytes"),
        ));
    }
    Ok(bytes)
}

fn load_authority(repository: &Path) -> ProducerResult<Authority> {
    let mut report = serde_json::Map::new();
    let mut provider_typeql = None;
    for (label, relative, expected_digest) in AUTHORITY_FILES {
        let bytes = bounded_regular_bytes(&repository.join(relative), label)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != expected_digest {
            return Err(ProducerError::new(
                "authority_hash_mismatch",
                format!("Phase-2 {label} is not the frozen Workforce V3 authority"),
            ));
        }
        report.insert(
            label.to_owned(),
            json!({"path": relative, "sha256": digest}),
        );
        if label == "provider" {
            provider_typeql = Some(String::from_utf8(bytes).map_err(|error| {
                ProducerError::caused(
                    "invalid_authority_utf8",
                    "Phase-2 provider authority is not UTF-8",
                    error,
                )
            })?);
        }
    }
    Ok(Authority {
        report: Value::Object(report),
        provider_typeql: provider_typeql.expect("provider is present in the fixed authority list"),
    })
}

fn require_generated_package() -> ProducerResult<()> {
    if SEMANTIC_SCHEMA_FINGERPRINT_JSON != EXPECTED_SEMANTIC_FINGERPRINT {
        return Err(ProducerError::new(
            "generated_package_mismatch",
            "generated crate is not the exact Workforce V3 semantic projection",
        ));
    }
    Ok(())
}

fn generated<T, E: fmt::Display>(result: Result<T, E>, context: &'static str) -> ProducerResult<T> {
    result.map_err(|error| ProducerError::caused("generated_value_rejected", context, error))
}

fn live<T, E: fmt::Display>(result: Result<T, E>, context: &'static str) -> ProducerResult<T> {
    result.map_err(|error| ProducerError::caused("live_operation_failed", context, error))
}

fn person_create(fixture: PersonFixture<'_>) -> ProducerResult<PersonCreate> {
    generated(
        PersonCreate::new(
            Vec::new(),
            Some(generated(FooBar::new(fixture.foo_bar), "person foo__bar")?),
            generated(Identifier::new(fixture.identifier), "person identifier")?,
            fixture
                .nickname
                .map(|value| generated(Nickname::new(value), "person nickname"))
                .transpose()?,
            generated(Score::new(fixture.score), "person score")?,
            Some(generated(
                ScoreGte::new(fixture.score_gte),
                "person score__gte",
            )?),
            generated(ValBool::new(fixture.val_bool), "person val_bool")?,
            generated(ValConstrained::new(fixture.score), "person val_constrained")?,
            generated(
                ValDate::new(generated(Date::try_new(fixture.date), "person date")?),
                "person val_date",
            )?,
            generated(
                ValDatetime::new(generated(
                    DateTime::try_new(fixture.datetime),
                    "person datetime",
                )?),
                "person val_datetime",
            )?,
            generated(
                ValDatetimeTz::new(generated(
                    DateTimeTz::try_new(fixture.datetime_tz),
                    "person timezone-aware datetime",
                )?),
                "person val_datetime_tz",
            )?,
            generated(
                ValDecimal::new(generated(
                    Decimal::try_new(fixture.decimal),
                    "person decimal",
                )?),
                "person val_decimal",
            )?,
            generated(
                ValDouble::new(generated(
                    CanonicalDouble::try_from_bits(fixture.double_bits),
                    "person double",
                )?),
                "person val_double",
            )?,
            generated(
                ValDuration::new(generated(
                    Duration::try_new(fixture.duration),
                    "person duration",
                )?),
                "person val_duration",
            )?,
        ),
        "person create",
    )
}

fn person_key(reference: &PersonRef, context: &'static str) -> ProducerResult<String> {
    reference
        .identifier()
        .map(|value| value.value().clone())
        .ok_or_else(|| ProducerError::new("missing_role_key", context))
}

fn robot_key(reference: &RobotRef, context: &'static str) -> ProducerResult<i64> {
    reference
        .robot_id()
        .map(|value| *value.value())
        .ok_or_else(|| ProducerError::new("missing_role_key", context))
}

fn require_hydrated_person_key(
    person: &Person,
    expected: &str,
    context: &'static str,
) -> ProducerResult<()> {
    if person.identifier().value() != expected {
        return Err(ProducerError::new("unexpected_live_value", context));
    }
    Ok(())
}

fn scalar_observation(person: &Person) -> ProducerResult<Value> {
    if person.identifier().value() != "data-ada"
        || person.nickname().map(Nickname::value).map(String::as_str) != Some("Ada")
        || *person.score().value() != 38
    {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "Ada scalar identity did not round trip",
        ));
    }
    Ok(json!({
        "hydrated": {
            "boolean": {"kind": "boolean", "value": *person.val_bool().value()},
            "date": {"kind": "date", "value": person.val_date().value().as_str()},
            "datetime": {"kind": "datetime", "value": person.val_datetime().value().as_str()},
            "datetime_tz": {"kind": "datetime_tz", "value": person.val_datetime_tz().value().as_str()},
            "decimal": {"kind": "decimal", "value": person.val_decimal().value().as_str()},
            "double": {"kind": "double", "bits": format!("{:016x}", person.val_double().value().to_bits())},
            "duration": {"kind": "duration", "value": person.val_duration().value().as_str()},
            "long": {"kind": "long", "value": person.score().value().to_string()},
            "string": {"kind": "string", "value": person.nickname().expect("Ada nickname was checked").value()},
        },
        "model": "person",
        "ref": "data-ada",
    }))
}

fn interaction_actor(
    relation_ref: &'static str,
    interaction: &Interaction,
) -> ProducerResult<Value> {
    let actor = match interaction.actor() {
        None => Value::Null,
        Some(InteractionActorPlayer::Person(reference)) => {
            let key = person_key(reference, "interaction person actor key")?;
            json!({"key": key, "model": "person"})
        }
        Some(InteractionActorPlayer::Robot(reference)) => {
            let key = robot_key(reference, "interaction robot actor key")?;
            json!({"key": key.to_string(), "model": "robot"})
        }
    };
    Ok(json!({"actor": actor, "relation_ref": relation_ref}))
}

async fn get_entity<M>(
    database: &GeneratedDatabase<AppSchema>,
    iid: &str,
    context: &'static str,
) -> ProducerResult<M>
where
    M: type_bridge::__codegen::EntityModel<Schema = AppSchema>
        + type_bridge::__codegen::CompleteModel,
{
    live(database.entities::<M>().get_by_iid(iid).await, context)?.ok_or_else(|| {
        ProducerError::new("missing_live_record", format!("{context} returned no row"))
    })
}

async fn get_relation<M>(
    database: &GeneratedDatabase<AppSchema>,
    iid: &str,
    context: &'static str,
) -> ProducerResult<M>
where
    M: type_bridge::__codegen::RelationModel<Schema = AppSchema>
        + type_bridge::__codegen::CompleteModel,
{
    live(database.relations::<M>().get_by_iid(iid).await, context)?.ok_or_else(|| {
        ProducerError::new("missing_live_record", format!("{context} returned no row"))
    })
}

async fn create_live_records(database: &GeneratedDatabase<AppSchema>) -> ProducerResult<Created> {
    let ada = live(
        database
            .entities::<Person>()
            .insert(person_create(PersonFixture {
                identifier: "data-ada",
                nickname: Some("Ada"),
                score: 38,
                foo_bar: 7,
                score_gte: 40,
                val_bool: false,
                date: "2026-08-12",
                datetime: "2026-08-12T09:30:00",
                datetime_tz: "2026-08-12T09:30:00Z",
                decimal: "38.5",
                double_bits: 0x4043_0000_0000_0000,
                duration: "PT38S",
            })?)
            .await,
        "insert Ada",
    )?;
    let dana = live(
        database
            .entities::<Person>()
            .insert(person_create(PersonFixture {
                identifier: "data-dana",
                nickname: None,
                score: 45,
                foo_bar: 9,
                score_gte: 40,
                val_bool: true,
                date: "2026-08-13",
                datetime: "2026-08-13T10:45:00",
                datetime_tz: "2026-08-13T10:45:00Z",
                decimal: "45.5",
                double_bits: 0x4046_8000_0000_0000,
                duration: "PT45S",
            })?)
            .await,
        "insert Dana",
    )?;
    let positive_robot = live(
        database
            .entities::<Robot>()
            .insert(generated(
                RobotCreate::new(
                    None,
                    generated(RobotId::new(7), "positive robot key")?,
                    generated(ValConstrained::new(12), "positive robot constrained value")?,
                ),
                "positive robot create",
            )?)
            .await,
        "insert positive robot",
    )?;
    let negative_robot = live(
        database
            .entities::<Robot>()
            .insert(generated(
                RobotCreate::new(
                    None,
                    generated(RobotId::new(-7), "negative robot key")?,
                    generated(ValConstrained::new(13), "negative robot constrained value")?,
                ),
                "negative robot create",
            )?)
            .await,
        "insert negative robot",
    )?;

    let interaction_robot = live(
        database
            .relations::<Interaction>()
            .insert(generated(
                InteractionCreate::new(
                    generated(
                        Identifier::new("data-interaction-robot"),
                        "robot interaction identifier",
                    )?,
                    None,
                    Some(InteractionActorRef::Robot(positive_robot.reference())),
                    ada.reference(),
                ),
                "robot interaction create",
            )?)
            .await,
        "insert robot interaction",
    )?;
    let interaction_absent = live(
        database
            .relations::<Interaction>()
            .insert(generated(
                InteractionCreate::new(
                    generated(
                        Identifier::new("data-interaction-absent"),
                        "absent interaction identifier",
                    )?,
                    None,
                    None,
                    dana.reference(),
                ),
                "absent interaction create",
            )?)
            .await,
        "insert absent interaction",
    )?;
    let interaction_person = live(
        database
            .relations::<Interaction>()
            .insert(generated(
                InteractionCreate::new(
                    generated(
                        Identifier::new("data-interaction-person"),
                        "person interaction identifier",
                    )?,
                    None,
                    Some(InteractionActorRef::Person(ada.reference())),
                    dana.reference(),
                ),
                "person interaction create",
            )?)
            .await,
        "insert person interaction",
    )?;
    let plain_activity = live(
        database
            .relations::<PlainActivity>()
            .insert(generated(
                PlainActivityCreate::new(ada.reference()),
                "plain activity create",
            )?)
            .await,
        "insert plain activity",
    )?;
    let event = live(
        database
            .relations::<Event>()
            .insert(generated(
                EventCreate::new(ada.reference()),
                "event create",
            )?)
            .await,
        "insert event",
    )?;
    let container = live(
        database
            .relations::<Container>()
            .insert(generated(
                ContainerCreate::new(vec![event.reference()]),
                "container create",
            )?)
            .await,
        "insert container",
    )?;

    Ok(Created {
        ada: ada.iid().to_owned(),
        dana: dana.iid().to_owned(),
        positive_robot: positive_robot.iid().to_owned(),
        negative_robot: negative_robot.iid().to_owned(),
        interaction_robot: interaction_robot.iid().to_owned(),
        interaction_absent: interaction_absent.iid().to_owned(),
        interaction_person: interaction_person.iid().to_owned(),
        plain_activity: plain_activity.iid().to_owned(),
        event: event.iid().to_owned(),
        container: container.iid().to_owned(),
    })
}

async fn require_pre_cleanup_counts(database: &GeneratedDatabase<AppSchema>) -> ProducerResult<()> {
    let counts = [
        (
            live(database.entities::<Person>().count().await, "count people")?,
            2,
        ),
        (
            live(database.entities::<Robot>().count().await, "count robots")?,
            2,
        ),
        (
            live(
                database.relations::<Interaction>().count().await,
                "count interactions",
            )?,
            3,
        ),
        (
            live(
                database.relations::<PlainActivity>().count().await,
                "count plain activities",
            )?,
            1,
        ),
        (
            live(database.relations::<Event>().count().await, "count events")?,
            1,
        ),
        (
            live(
                database.relations::<Container>().count().await,
                "count containers",
            )?,
            1,
        ),
    ];
    if counts.iter().any(|(actual, expected)| actual != expected) {
        return Err(ProducerError::new(
            "unexpected_live_count",
            "created live subset counts differ from the journey",
        ));
    }
    Ok(())
}

async fn observe_live_records(
    database: &GeneratedDatabase<AppSchema>,
    created: &Created,
) -> ProducerResult<(Value, Value, Value, Value)> {
    require_pre_cleanup_counts(database).await?;

    let ada = get_entity::<Person>(database, &created.ada, "read Ada").await?;
    let dana = get_entity::<Person>(database, &created.dana, "read Dana").await?;
    if dana.identifier().value() != "data-dana" {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "Dana identity did not round trip",
        ));
    }
    let canonical_scalars = scalar_observation(&ada)?;

    let negative_robot =
        get_entity::<Robot>(database, &created.negative_robot, "read negative robot").await?;
    let positive_robot =
        get_entity::<Robot>(database, &created.positive_robot, "read positive robot").await?;
    if *negative_robot.robot_id().value() != -7 || *positive_robot.robot_id().value() != 7 {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "signed robot keys did not round trip",
        ));
    }

    let absent = get_relation::<Interaction>(
        database,
        &created.interaction_absent,
        "read absent interaction",
    )
    .await?;
    let person = get_relation::<Interaction>(
        database,
        &created.interaction_person,
        "read person interaction",
    )
    .await?;
    let robot = get_relation::<Interaction>(
        database,
        &created.interaction_robot,
        "read robot interaction",
    )
    .await?;
    let states = vec![
        interaction_actor("interaction-absent", &absent)?,
        interaction_actor("interaction-person", &person)?,
        interaction_actor("interaction-robot", &robot)?,
    ];
    if states
        != vec![
            json!({"actor": null, "relation_ref": "interaction-absent"}),
            json!({"actor": {"key": "data-ada", "model": "person"}, "relation_ref": "interaction-person"}),
            json!({"actor": {"key": "7", "model": "robot"}, "relation_ref": "interaction-robot"}),
        ]
    {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "polymorphic optional interaction actor states drifted",
        ));
    }
    let integer_and_optional = json!({
        "integer_keys": [
            {"model": "robot", "ref": "robot-negative-7", "value": negative_robot.robot_id().value().to_string()},
            {"model": "robot", "ref": "robot-7", "value": positive_robot.robot_id().value().to_string()},
        ],
        "optional_role": "actor",
        "relation": "interaction",
        "states": states,
    });

    let plain =
        get_relation::<PlainActivity>(database, &created.plain_activity, "read plain activity")
            .await?;
    require_hydrated_person_key(
        plain.participant(),
        "data-ada",
        "plain activity participant identity",
    )?;
    let plain_before_cleanup = json!({
        "created": true,
        "inherited_relation": "base-activity",
        "model": "plain-activity",
        "participant": {"key": "data-ada", "model": "person"},
        "read_after_create": true,
        "ref": "plain-activity-ada",
        "role": "participant",
        "role_identity_preserved": true,
    });

    let event = get_relation::<Event>(database, &created.event, "read event").await?;
    require_hydrated_person_key(
        event.subject(),
        "data-ada",
        "event subject identity",
    )?;
    let container =
        get_relation::<Container>(database, &created.container, "read container").await?;
    let [ContainerItemPlayer::Event(item)] = container.item() else {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "container did not hydrate exactly one Event player",
        ));
    };
    if item.iid() != Some(created.event.as_str()) {
        return Err(ProducerError::new(
            "unexpected_live_value",
            "container Event player lost its relation identity",
        ));
    }
    let relation_as_player = json!({
        "owner": {"model": "container", "ref": "container-event"},
        "player": {"model": "event", "ref": "event-ada"},
        "preserved": true,
        "role": "item",
    });

    Ok((
        canonical_scalars,
        integer_and_optional,
        plain_before_cleanup,
        relation_as_player,
    ))
}

async fn require_entity_absent<M>(
    database: &GeneratedDatabase<AppSchema>,
    iid: &str,
    context: &'static str,
) -> ProducerResult<()>
where
    M: type_bridge::__codegen::EntityModel<Schema = AppSchema>
        + type_bridge::__codegen::CompleteModel,
{
    if live(database.entities::<M>().get_by_iid(iid).await, context)?.is_some() {
        return Err(ProducerError::new("delete_not_observed", context));
    }
    Ok(())
}

async fn require_relation_absent<M>(
    database: &GeneratedDatabase<AppSchema>,
    iid: &str,
    context: &'static str,
) -> ProducerResult<()>
where
    M: type_bridge::__codegen::RelationModel<Schema = AppSchema>
        + type_bridge::__codegen::CompleteModel,
{
    if live(database.relations::<M>().get_by_iid(iid).await, context)?.is_some() {
        return Err(ProducerError::new("delete_not_observed", context));
    }
    Ok(())
}

async fn cleanup_live_records(
    database: &GeneratedDatabase<AppSchema>,
    created: &Created,
) -> ProducerResult<(Value, bool, u64)> {
    let mut order = Vec::with_capacity(10);

    live(
        database
            .relations::<Container>()
            .delete(&created.container)
            .await,
        "delete container",
    )?;
    require_relation_absent::<Container>(database, &created.container, "read deleted container")
        .await?;
    order.push("container-event");

    live(
        database.relations::<Event>().delete(&created.event).await,
        "delete event",
    )?;
    require_relation_absent::<Event>(database, &created.event, "read deleted event").await?;
    order.push("event-ada");

    live(
        database
            .relations::<PlainActivity>()
            .delete(&created.plain_activity)
            .await,
        "delete plain activity",
    )?;
    let read_after_delete = live(
        database
            .relations::<PlainActivity>()
            .get_by_iid(&created.plain_activity)
            .await,
        "read deleted plain activity",
    )?
    .is_some();
    if read_after_delete {
        return Err(ProducerError::new(
            "delete_not_observed",
            "plain activity remained after delete",
        ));
    }
    let plain_count = live(
        database.relations::<PlainActivity>().count().await,
        "count plain activities after delete",
    )?;
    if plain_count != 0 {
        return Err(ProducerError::new(
            "unexpected_live_count",
            "plain activity count is not zero after delete",
        ));
    }
    order.push("plain-activity-ada");

    for (iid, reference, context) in [
        (
            created.interaction_person.as_str(),
            "interaction-person",
            "delete person interaction",
        ),
        (
            created.interaction_absent.as_str(),
            "interaction-absent",
            "delete absent interaction",
        ),
        (
            created.interaction_robot.as_str(),
            "interaction-robot",
            "delete robot interaction",
        ),
    ] {
        live(
            database.relations::<Interaction>().delete(iid).await,
            context,
        )?;
        require_relation_absent::<Interaction>(database, iid, "read deleted interaction").await?;
        order.push(reference);
    }

    live(
        database
            .entities::<Robot>()
            .delete(&created.negative_robot)
            .await,
        "delete negative robot",
    )?;
    require_entity_absent::<Robot>(
        database,
        &created.negative_robot,
        "read deleted negative robot",
    )
    .await?;
    order.push("robot-negative-7");

    live(
        database
            .entities::<Robot>()
            .delete(&created.positive_robot)
            .await,
        "delete positive robot",
    )?;
    require_entity_absent::<Robot>(
        database,
        &created.positive_robot,
        "read deleted positive robot",
    )
    .await?;
    order.push("robot-7");

    live(
        database.entities::<Person>().delete(&created.dana).await,
        "delete Dana",
    )?;
    require_entity_absent::<Person>(database, &created.dana, "read deleted Dana").await?;
    order.push("data-dana");

    live(
        database.entities::<Person>().delete(&created.ada).await,
        "delete Ada",
    )?;
    require_entity_absent::<Person>(database, &created.ada, "read deleted Ada").await?;
    order.push("data-ada");

    let zero_checks = vec![
        json!({"count_after_cleanup": live(database.relations::<Container>().count().await, "final container count")?, "model": "container"}),
        json!({"count_after_cleanup": live(database.relations::<Event>().count().await, "final event count")?, "model": "event"}),
        json!({"count_after_cleanup": live(database.relations::<Interaction>().count().await, "final interaction count")?, "model": "interaction"}),
        json!({"count_after_cleanup": live(database.entities::<Person>().count().await, "final person count")?, "model": "person"}),
        json!({"count_after_cleanup": live(database.relations::<PlainActivity>().count().await, "final plain activity count")?, "model": "plain-activity"}),
        json!({"count_after_cleanup": live(database.entities::<Robot>().count().await, "final robot count")?, "model": "robot"}),
    ];
    if zero_checks
        .iter()
        .any(|item| item["count_after_cleanup"] != 0)
    {
        return Err(ProducerError::new(
            "unexpected_live_count",
            "one or more live models remain after cleanup",
        ));
    }

    Ok((
        json!({"order": order, "zero_checks": zero_checks}),
        false,
        plain_count,
    ))
}

async fn run_journey(database: &GeneratedDatabase<AppSchema>) -> ProducerResult<Value> {
    let created = create_live_records(database).await?;
    let (canonical_scalars, integer_and_optional, mut plain, relation_as_player) =
        observe_live_records(database, &created).await?;
    let (cleanup, read_after_delete, plain_count) =
        cleanup_live_records(database, &created).await?;
    let plain_object = plain
        .as_object_mut()
        .expect("plain observation is constructed as an object");
    plain_object.insert("count_after_delete".to_owned(), json!(plain_count));
    plain_object.insert("deleted".to_owned(), json!(true));
    plain_object.insert("read_after_delete".to_owned(), json!(read_after_delete));

    Ok(json!({
        "canonical_scalar_values": canonical_scalars,
        "cleanup": cleanup,
        "inherited_plain_activity_role_lifecycle": plain,
        "integer_key_polymorphic_optional_role": integer_and_optional,
        "relation_as_player": relation_as_player,
    }))
}

async fn execute_live(config: &Config, provider_typeql: &str) -> ProducerResult<Value> {
    let admin_options = ConnectOptions {
        http_port: config.http_port,
        tls: false,
        ..ConnectOptions::default()
    };
    let admin = live(
        AdminDatabase::connect_with_options(
            &config.address,
            &config.database,
            USERNAME,
            PASSWORD,
            admin_options,
        )
        .await,
        "connect administrative database handle",
    )?;
    let mut owns_database = false;

    let primary = async {
        let detected = admin.server_version().ok_or_else(|| {
            ProducerError::new(
                "missing_server_version",
                "provider did not expose an authoritative server version",
            )
        })?;
        if detected.to_string() != SERVER_VERSION {
            return Err(ProducerError::new(
                "provider_version_mismatch",
                format!("expected {SERVER_VERSION}, detected {detected}"),
            ));
        }
        if live(
            admin.database_exists().await,
            "check initial database absence",
        )? {
            return Err(ProducerError::new(
                "database_already_exists",
                "caller-supplied live database must initially be absent",
            ));
        }
        live(admin.create_database().await, "create live database")?;
        owns_database = true;
        live(
            admin.execute_raw(provider_typeql, TxType::Schema).await,
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
            "connect generated Rust database",
        )?;
        let database = live(
            unbound.with_schema(SCHEMA),
            "bind generated Workforce V3 package",
        )?;
        let observations = run_journey(&database).await;
        drop(database);
        observations
    }
    .await;

    let delete_result = if owns_database {
        live(admin.delete_database().await, "delete owned live database")
    } else {
        Ok(())
    };
    let close_result = live(admin.close(), "close administrative database handle");

    match (primary, delete_result, close_result) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (Err(primary), Ok(()), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup), Ok(())) | (Err(primary), Ok(()), Err(cleanup)) => {
            Err(primary.with_cleanup(&cleanup))
        }
        (Err(primary), Err(delete), Err(close)) => {
            Err(primary.with_cleanup(&delete).with_cleanup(&close))
        }
        (Ok(_), Err(cleanup), Ok(())) | (Ok(_), Ok(()), Err(cleanup)) => Err(cleanup),
        (Ok(_), Err(delete), Err(close)) => Err(delete.with_cleanup(&close)),
    }
}

fn sorted_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sorted_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, sorted_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

fn canonical_report_bytes(report: Value) -> ProducerResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&sorted_json(report)).map_err(|error| {
        ProducerError::caused(
            "report_serialization_failed",
            "report is not canonical JSON",
            error,
        )
    })?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(ProducerError::new(
            "report_size_limit",
            format!("report exceeds {MAX_REPORT_BYTES} bytes"),
        ));
    }
    Ok(bytes)
}

fn publish_create_new(path: &Path, bytes: &[u8]) -> ProducerResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            ProducerError::caused(
                "report_publication_failed",
                format!("{} cannot be created", path.display()),
                error,
            )
        })?;
    let publication = (|| {
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = publication {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(ProducerError::caused(
            "report_publication_failed",
            format!("{} could not be written completely", path.display()),
            error,
        ));
    }
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(ProducerError::caused(
                "report_publication_failed",
                "published report cannot be inspected",
                error,
            ));
        }
    };
    if !metadata.is_file() || metadata.len() as usize != bytes.len() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(ProducerError::new(
            "report_publication_failed",
            "published report is not one complete regular file",
        ));
    }
    Ok(())
}

async fn run() -> ProducerResult<()> {
    require_generated_package()?;
    let config = parse_config()?;
    let authority = load_authority(&config.repository)?;
    let observations = execute_live(&config, &authority.provider_typeql).await?;
    let report = json!({
        "authority": authority.report,
        "binding": "rust",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    });
    let bytes = canonical_report_bytes(report)?;
    publish_create_new(&config.output, &bytes)
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Phase-2 Rust live producer rejected: {error}");
        std::process::exit(1);
    }
}
