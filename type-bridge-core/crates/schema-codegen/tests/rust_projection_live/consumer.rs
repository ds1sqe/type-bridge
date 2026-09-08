use std::env;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use type_bridge::value::{
    Date as QueryDate, DateTime as QueryDateTime, DateTimeTz as QueryDateTimeTz,
    Decimal as QueryDecimal, Double as QueryDouble, Regex, Text,
};
use type_bridge::{
    ConnectionOptions, Database, PageOptions, RemoteConnectionOptions, RemoteDatabase,
    RemoteQueryLimits, RemoteQueryTransport, RowsOptions, aggregate,
};
use type_bridge_generated_schema::{
    Actor, ActorType, Aliases, AppSchema, CanonicalDouble, Container, ContainerCreate,
    ContainerType, Contractor, ContractorCode, ContractorCreate, Counter, CounterCreate,
    CounterType, CounterValue, Date, DateTime, DateTimeTz, Decimal, Duration, Employee,
    EmployeeCreate, EmployeeFamily, Employment, EmploymentCreate, EmploymentType, Event,
    EventCreate, EventType, FooBar, Identifier, Interaction, InteractionActorPlayer,
    InteractionActorRef, InteractionCreate, InteractionTargetPlayer, InteractionType, Manager,
    ManagerCreate, ManagerNote, Membership, MembershipCreate, MembershipFamily,
    MembershipMemberPlayer, MembershipMemberRef, MembershipType, NetworkLink, NetworkLinkCreate,
    NetworkLinkDestinationPlayer, NetworkLinkOriginPlayer, NetworkLinkType, Nickname, Party,
    PartyFamily, PartyName, Person, PersonCreate, PersonRef, PersonType, PlainActivity,
    PlainActivityCreate, PlainActivityParticipantPlayer, PlainActivityType, Rank, Robot,
    RobotCreate, RobotId, RobotType, SCHEMA, Score, ScoreGte, ValBool, ValConstrained, ValDate,
    ValDatetime, ValDatetimeTz, ValDecimal, ValDouble, ValDuration, plays_event_container_item,
};

#[derive(type_bridge::SelectedRow)]
struct PersonGraph {
    person: Person,
    members: Vec<Person>,
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
}

impl HttpTransport {
    fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
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
    match keyed_put.destination() {
        NetworkLinkDestinationPlayer::Person(value) => {
            assert_eq!(
                value.identifier().expect("destination key").value(),
                "p-202"
            )
        }
    }
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
    match updated_new.origin() {
        NetworkLinkOriginPlayer::Person(value) => {
            assert_eq!(value.identifier().expect("origin key").value(), "p-202")
        }
    }
    match updated_new.destination() {
        NetworkLinkDestinationPlayer::Person(value) => {
            assert_eq!(
                value.identifier().expect("destination key").value(),
                "p-203"
            )
        }
    }
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
    let remote: RemoteDatabase<AppSchema> = RemoteDatabase::connect(
        RemoteConnectionOptions::generated(
        RemoteQueryLimits::new(100, 8 << 20, 1000, 1000, 1000, 1000).deadline_ms(30_000),
        HttpTransport::new(remote_url),
        ),
    )
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
    match robot_interaction.target() {
        InteractionTargetPlayer::Person(reference) => {
            assert_eq!(
                reference.identifier().expect("target key").value(),
                "parity-person-target"
            );
        }
    }

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
    match surviving.target() {
        InteractionTargetPlayer::Person(reference) => {
            assert_eq!(reference.iid(), Some(target.iid()));
        }
    }

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
    match activity.participant() {
        PlainActivityParticipantPlayer::Person(reference) => {
            assert_eq!(reference.iid(), Some(person.iid()));
        }
    }

    let stored = db
        .relations::<PlainActivity>()
        .get_by_iid(activity.iid())
        .await
        .expect("plain activity lookup")
        .expect("plain activity exists");
    match stored.participant() {
        PlainActivityParticipantPlayer::Person(reference) => {
            assert_eq!(reference.iid(), Some(person.iid()));
        }
    }

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
