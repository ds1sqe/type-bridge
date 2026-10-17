//! The major TypeBridge pipelines through the public Rust SDK, against a
//! live TypeDB, timed end to end and per stage.
//!
//! ```text
//! type-bridge-perf-pipelines --out FILE [--iterations N] [--warmup N]
//!     [--migration-iterations N] [--pipeline NAME]... [--work-dir DIR]
//! type-bridge-perf-pipelines --binding --out FILE [--iterations N] [--warmup N]
//! ```
//!
//! TypeDB comes from `TYPEDB_ADDRESS` (default `localhost:1729`) and
//! `TYPEDB_HTTP_PORT` (default `8000`); `TYPEDB_USERNAME` and
//! `TYPEDB_PASSWORD` are required, because the migration workspace names
//! them as `env:` credentials.
//!
//! Every pipeline gets a fresh database whose schema the in-process CLI
//! migration applies, then runs warmup plus measured iterations one at a
//! time. The report holds each measured iteration's wall time and the
//! exclusive time of every stage span inside it (see
//! `type_bridge_perf::stages`); `python -m perf.harness.pipeline` derives
//! the percentiles. Databases are deleted when their pipeline ends.
//!
//! `--binding` instead times the offline binding ops (`BINDING_OPS`) with
//! no TypeDB and writes a `type-bridge-perf-binding/v1` report, the Rust
//! baseline `python -m perf.harness.binding` subtracts from the bindings.

#[path = "../../../rust/tests/support/purchase_models.rs"]
mod purchase_models;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use purchase_models::{
    Account, AccountType, Doctor, DoctorType, Item, ItemType, Purchase, PurchaseType, Shop,
};
use serde_json::{Value as Json, json};
use type_bridge::plan::Plan;
use type_bridge::staged::{PlanAnswer, TransactionKind, TransactionOptions};
use type_bridge::{ConnectionOptions, Database, SchemaPackage};
use type_bridge_perf::stages::{self, StageTimes};

/// The report format `perf.harness.pipeline` reads.
const FORMAT: &str = "type-bridge-perf-pipelines/v1";
const BINDING_FORMAT: &str = "type-bridge-perf-binding/v1";
/// The offline `--binding` ops, in report order: build the purchase plan,
/// build the 1k-row insert (encoding its given rows), and encode that plan's
/// canonical bytes. None opens a database: staging needs a connected one.
const BINDING_OPS: [&str; 3] = ["build_purchase", "build_rows_1k", "encode_rows_1k"];
/// Items seeded for the key, filter, update pipelines.
const SEEDED_ITEMS: i64 = 1_000;
/// Seeded items with stock at or above this match the filtered fetch.
const FILTER_STOCK: i64 = 900;
const BATCH_ROWS: usize = 1_000;
const MIXED_BATCH_ROWS: usize = 50;
const DOCTORS: usize = 10;

/// The pipelines, in run order, with the logical operations one
/// iteration performs (for throughput).
const PIPELINES: [(&str, usize); 8] = [
    ("insert_one", 1),
    ("fetch_by_key", 1),
    ("fetch_filtered", 1),
    ("update", 1),
    ("delete", 1),
    ("batch_given_rows_1k", BATCH_ROWS),
    ("staged_mixed", 1),
    ("migration_apply", 1),
];

#[derive(Debug)]
enum Failure {
    Usage(String),
    Sdk(type_bridge::Error),
    Orm(type_bridge_orm::OrmError),
    Migration(String),
    Answer(String),
    Io(std::io::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(formatter, "usage: {message}"),
            Self::Sdk(error) => write!(formatter, "sdk: {error}"),
            Self::Orm(error) => write!(formatter, "database admin: {error}"),
            Self::Migration(message) => write!(formatter, "migration: {message}"),
            Self::Answer(message) => write!(formatter, "unexpected answer: {message}"),
            Self::Io(error) => write!(formatter, "io: {error}"),
        }
    }
}

impl From<type_bridge::Error> for Failure {
    fn from(error: type_bridge::Error) -> Self {
        Self::Sdk(error)
    }
}

impl From<type_bridge::staged::PendingError> for Failure {
    fn from(error: type_bridge::staged::PendingError) -> Self {
        Self::Sdk(error.into())
    }
}

impl From<type_bridge::staged::CommitError> for Failure {
    fn from(error: type_bridge::staged::CommitError) -> Self {
        Self::Sdk(error.into())
    }
}

impl From<type_bridge_orm::OrmError> for Failure {
    fn from(error: type_bridge_orm::OrmError) -> Self {
        Self::Orm(error)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T, E = Failure> = std::result::Result<T, E>;

struct Config {
    out: PathBuf,
    work_dir: PathBuf,
    iterations: usize,
    warmup: usize,
    migration_iterations: usize,
    only: Vec<String>,
    binding: bool,
    address: String,
    http_port: u16,
    username: String,
    password: String,
}

impl Config {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self> {
        let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let mut config = Self {
            out: PathBuf::new(),
            work_dir: std::env::temp_dir().join(format!("type-bridge-perf-{}", std::process::id())),
            iterations: 30,
            warmup: 5,
            migration_iterations: 5,
            only: Vec::new(),
            binding: false,
            address: env("TYPEDB_ADDRESS").unwrap_or_else(|| "localhost:1729".into()),
            http_port: env("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|| "8000".into())
                .parse()
                .map_err(|_| Failure::Usage("TYPEDB_HTTP_PORT must be a port".into()))?,
            username: env("TYPEDB_USERNAME").unwrap_or_default(),
            password: env("TYPEDB_PASSWORD").unwrap_or_default(),
        };
        let count = |value: Option<String>, flag: &str| {
            value
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|count| *count > 0)
                .ok_or_else(|| Failure::Usage(format!("{flag} takes a positive count")))
        };
        while let Some(flag) = arguments.next() {
            match flag.as_str() {
                "--out" => config.out = arguments.next().map(PathBuf::from).unwrap_or_default(),
                "--work-dir" => {
                    config.work_dir = arguments.next().map(PathBuf::from).unwrap_or_default();
                }
                "--iterations" => config.iterations = count(arguments.next(), &flag)?,
                "--warmup" => config.warmup = count(arguments.next(), &flag)?,
                "--binding" => config.binding = true,
                "--migration-iterations" => {
                    config.migration_iterations = count(arguments.next(), &flag)?;
                }
                "--pipeline" => {
                    let name = arguments.next().unwrap_or_default();
                    if !PIPELINES.iter().any(|(known, _)| *known == name) {
                        return Err(Failure::Usage(format!("unknown pipeline {name:?}")));
                    }
                    config.only.push(name);
                }
                other => return Err(Failure::Usage(format!("unknown argument {other:?}"))),
            }
        }
        if config.out.as_os_str().is_empty() {
            return Err(Failure::Usage("--out FILE is required".into()));
        }
        if !config.binding && (config.username.is_empty() || config.password.is_empty()) {
            return Err(Failure::Usage(
                "set TYPEDB_USERNAME and TYPEDB_PASSWORD for the TypeDB login".into(),
            ));
        }
        Ok(config)
    }

    fn selected(&self, name: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|only| only == name)
    }
}

/// One measured iteration: wall time and exclusive stage times.
struct Sample {
    total_ns: u64,
    stages: BTreeMap<&'static str, u64>,
}

fn nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// A split-YAML workspace of the purchase schema with one migration, whose
/// environment is pointed at a new database for every apply.
struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn create(config: &Config) -> Result<Self> {
        let root = config.work_dir.join("workspace");
        if root.exists() {
            return Err(Failure::Usage(format!(
                "work directory {} already holds a workspace",
                root.display()
            )));
        }
        std::fs::create_dir_all(root.join("schema/fragments"))?;
        std::fs::write(
            root.join("schema/schema.yaml"),
            "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
        )?;
        std::fs::write(
            root.join("schema/fragments/purchase.yaml"),
            purchase_models::SOURCE,
        )?;
        let workspace = Self { root };
        workspace.point_at(config, "unused")?;
        workspace.cli(&["migration", "make", "--name", "initial"])?;
        Ok(workspace)
    }

    fn manifest(&self) -> PathBuf {
        self.root.join("typebridge.yaml")
    }

    /// Write the manifest with its one environment naming `database`.
    fn point_at(&self, config: &Config, database: &str) -> Result<()> {
        let manifest = format!(
            "format: typebridge.workspace/v1
schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: {scope}
compatibility:
  semantic-profile: {profile}
migrations:
  directory: migrations/v2
  app-label: perf
environments:
  perf:
    database: {database}
    uri: {address}
    http-port: '{port}'
    tls: 'false'
    migrate: 'true'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
",
            scope = purchase_models::SCOPE,
            profile = purchase_models::PROFILE,
            address = config.address,
            port = config.http_port,
        );
        std::fs::write(self.manifest(), manifest)?;
        Ok(())
    }

    /// Run one CLI command in this process, so its spans are recorded.
    fn cli(&self, command: &[&str]) -> Result<()> {
        let manifest = self.manifest();
        let mut arguments = vec!["type-bridge", "--manifest"];
        arguments.push(
            manifest.to_str().ok_or_else(|| {
                Failure::Usage("the work directory path must be valid UTF-8".into())
            })?,
        );
        arguments.extend_from_slice(command);
        match type_bridge_cli::run_cli(arguments) {
            0 => Ok(()),
            code => Err(Failure::Migration(format!(
                "`type-bridge {}` exited with {code}",
                command.join(" ")
            ))),
        }
    }

    /// Create `database` with the purchase schema by applying the chain.
    fn apply(&self, config: &Config, database: &str) -> Result<()> {
        self.point_at(config, database)?;
        self.cli(&["migration", "apply", "--environment", "perf"])
    }
}

/// Delete a managed database and its migration journal.
async fn drop_databases(config: &Config, database: &str) -> Result<()> {
    for name in [database.to_owned(), format!("{database}__tbv2_journal")] {
        let admin = type_bridge_orm::Database::connect_with_options(
            &config.address,
            &name,
            &config.username,
            &config.password,
            type_bridge_orm::ConnectOptions {
                http_port: config.http_port,
                ..type_bridge_orm::ConnectOptions::default()
            },
        )
        .await?;
        if admin.database_exists().await? {
            admin.delete_database().await?;
        }
    }
    Ok(())
}

/// The connected SDK database and the package plans build against.
struct Target {
    db: Database<Shop>,
    package: SchemaPackage<Shop>,
}

type Built = Plan<Shop>;

fn write_kind() -> TransactionOptions {
    TransactionOptions::default().with_kind(TransactionKind::Write)
}

fn read_kind() -> TransactionOptions {
    TransactionOptions::default().with_kind(TransactionKind::Read)
}

/// Stage `plans` in one transaction, commit, and return their answers.
async fn commit(
    target: &Target,
    options: TransactionOptions,
    plans: &[&Built],
) -> Result<Vec<PlanAnswer>> {
    let mut tx = target.db.transaction(options);
    let pending = plans
        .iter()
        .map(|plan| tx.stage(plan))
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    pending
        .iter()
        .map(|answer| Ok((*answer.get()?).clone()))
        .collect()
}

fn expect(answer: &PlanAnswer, rows: usize) -> Result<()> {
    let actual = match answer {
        PlanAnswer::Matched(count) | PlanAnswer::Count(count) => {
            usize::try_from(*count).unwrap_or(usize::MAX)
        }
        PlanAnswer::Rows(found) => found.len(),
        other => return Err(Failure::Answer(format!("{other:?}"))),
    };
    if actual == rows {
        Ok(())
    } else {
        Err(Failure::Answer(format!("{actual} rows, expected {rows}")))
    }
}

/// Insert items `{prefix}{n}` with stock `n` for each `n` in `numbers`.
fn insert_items(
    package: &SchemaPackage<Shop>,
    prefix: &str,
    numbers: impl Iterator<Item = i64>,
) -> Result<Built> {
    let mut plan = Plan::builder();
    let id = plan.input::<String>("item_id");
    let stock = plan.input::<i64>("stock");
    let price = plan.input::<i64>("price");
    plan.insert::<Item>()
        .field(ItemType::item_id, id)
        .field(ItemType::stock, stock)
        .field(ItemType::price, price);
    for number in numbers {
        plan.row([
            id.value(format!("{prefix}{number}")),
            stock.value(number),
            price.value(5),
        ]);
    }
    Ok(plan.build(package)?)
}

fn fetch_by_key(package: &SchemaPackage<Shop>, key: String) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let id = plan.input::<String>("item_id");
    let item_key = plan.field(item, ItemType::item_id);
    let stock = plan.field(item, ItemType::stock);
    let price = plan.field(item, ItemType::price);
    plan.where_(item_key.eq(id)).returning([stock, price]);
    plan.row([id.value(key)]);
    Ok(plan.build(package)?)
}

fn fetch_filtered(package: &SchemaPackage<Shop>) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let item_key = plan.field(item, ItemType::item_id);
    let stock = plan.field(item, ItemType::stock);
    plan.where_(stock.ge(FILTER_STOCK))
        .returning([item_key, stock]);
    Ok(plan.build(package)?)
}

fn restock(package: &SchemaPackage<Shop>, key: String) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let id = plan.input::<String>("item_id");
    let item_key = plan.field(item, ItemType::item_id);
    let stock = plan.field(item, ItemType::stock);
    plan.where_(item_key.eq(id));
    let next = plan.let_("next_stock", stock + 1_i64);
    plan.update(item, ItemType::stock, next);
    plan.row([id.value(key)]);
    Ok(plan.build(package)?)
}

fn delete_item(package: &SchemaPackage<Shop>, key: String) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let id = plan.input::<String>("item_id");
    let item_key = plan.field(item, ItemType::item_id);
    plan.where_(item_key.eq(id)).delete(item);
    plan.row([id.value(key)]);
    Ok(plan.build(package)?)
}

/// One stocked item and one funded account, enough for every purchase.
fn owners(package: &SchemaPackage<Shop>) -> Result<Built> {
    let mut plan = Plan::builder();
    plan.insert::<Item>()
        .field(ItemType::item_id, "I1".to_owned())
        .field(ItemType::stock, 1_000_000_i64)
        .field(ItemType::price, 1_i64);
    plan.insert::<Account>()
        .field(AccountType::account_id, "A1".to_owned())
        .field(AccountType::balance, 1_000_000_000_i64);
    Ok(plan.build(package)?)
}

/// The on-call doctors the touch rewrites.
fn doctors(package: &SchemaPackage<Shop>) -> Result<Built> {
    let mut plan = Plan::builder();
    let name = plan.input::<String>("name");
    plan.insert::<Doctor>()
        .field(DoctorType::name, name)
        .field(DoctorType::on_call, true);
    for number in 0..DOCTORS {
        plan.row([name.value(format!("D{number}"))]);
    }
    Ok(plan.build(package)?)
}

/// The one-query purchase of `I1` by `A1`.
fn purchase(package: &SchemaPackage<Shop>, order: String) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_key = plan.field(item, ItemType::item_id);
    let buyer_key = plan.field(buyer, AccountType::account_id);
    let stock = plan.field(item, ItemType::stock);
    let price = plan.field(item, ItemType::price);
    let balance = plan.field(buyer, AccountType::balance);
    plan.where_(item_key.eq("I1".to_owned()))
        .where_(buyer_key.eq("A1".to_owned()))
        .where_(stock.gt(0_i64))
        .where_(balance.ge(price));
    let next_stock = plan.let_("next_stock", stock - 1_i64);
    let next_balance = plan.let_("next_balance", balance - price);
    plan.update(item, ItemType::stock, next_stock).update(
        buyer,
        AccountType::balance,
        next_balance,
    );
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    plan.row([order_input.value(order)]);
    Ok(plan.build(package)?)
}

/// Rewrite every doctor's on-call value with itself, taking its lock.
fn touch(package: &SchemaPackage<Shop>) -> Result<Built> {
    let mut plan = Plan::builder();
    let doctor = plan.exact::<Doctor>();
    let on_call = plan.field(doctor, DoctorType::on_call);
    plan.update(doctor, DoctorType::on_call, on_call);
    Ok(plan.build(package)?)
}

/// Orders `{prefix}{n}` of `I1` by `A1`, one given row each.
fn orders(package: &SchemaPackage<Shop>, prefix: &str, rows: usize) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_key = plan.field(item, ItemType::item_id);
    plan.where_(item_key.eq("I1".to_owned()));
    let buyer_key = plan.field(buyer, AccountType::account_id);
    plan.where_(buyer_key.eq("A1".to_owned()));
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    for number in 0..rows {
        plan.row([order_input.value(format!("{prefix}{number}"))]);
    }
    Ok(plan.build(package)?)
}

/// Seed what the pipeline's iterations read, change or delete.
async fn seed(name: &str, target: &Target, iterations: usize) -> Result<()> {
    let package = &target.package;
    match name {
        "fetch_by_key" | "fetch_filtered" | "update" => {
            let items = insert_items(package, "item-", 0..SEEDED_ITEMS)?;
            commit(target, write_kind(), &[&items]).await?;
        }
        "delete" => {
            let count = i64::try_from(iterations).unwrap_or(i64::MAX);
            let items = insert_items(package, "doomed-", 0..count)?;
            commit(target, write_kind(), &[&items]).await?;
        }
        "staged_mixed" => {
            let owners = owners(package)?;
            let doctors = doctors(package)?;
            commit(target, write_kind(), &[&owners, &doctors]).await?;
        }
        _ => {}
    }
    Ok(())
}

/// One iteration of a data pipeline: build its plans, stage, commit and
/// check the answers. Iteration numbers are unique within a pipeline.
async fn iteration(name: &str, target: &Target, number: usize) -> Result<()> {
    let package = &target.package;
    let key = |prefix: &str| {
        let seeded = usize::try_from(SEEDED_ITEMS).unwrap_or(usize::MAX);
        format!("{prefix}{}", number % seeded)
    };
    match name {
        "insert_one" => {
            let at = i64::try_from(number).unwrap_or(i64::MAX);
            let plan = insert_items(package, "new-", at..at + 1)?;
            expect(&commit(target, write_kind(), &[&plan]).await?[0], 1)
        }
        "fetch_by_key" => {
            let plan = fetch_by_key(package, key("item-"))?;
            expect(&commit(target, read_kind(), &[&plan]).await?[0], 1)
        }
        "fetch_filtered" => {
            let plan = fetch_filtered(package)?;
            let rows = usize::try_from(SEEDED_ITEMS - FILTER_STOCK).unwrap_or(0);
            expect(&commit(target, read_kind(), &[&plan]).await?[0], rows)
        }
        "update" => {
            let plan = restock(package, key("item-"))?;
            expect(&commit(target, write_kind(), &[&plan]).await?[0], 1)
        }
        "delete" => {
            let plan = delete_item(package, format!("doomed-{number}"))?;
            expect(&commit(target, write_kind(), &[&plan]).await?[0], 1)
        }
        "batch_given_rows_1k" => {
            let rows = i64::try_from(BATCH_ROWS).unwrap_or(i64::MAX);
            let plan = insert_items(package, &format!("batch-{number}-"), 0..rows)?;
            expect(
                &commit(target, write_kind(), &[&plan]).await?[0],
                BATCH_ROWS,
            )
        }
        "staged_mixed" => {
            let bought = purchase(package, format!("O{number}"))?;
            let touched = touch(package)?;
            let batch = orders(package, &format!("B{number}-"), MIXED_BATCH_ROWS)?;
            let answers = commit(target, write_kind(), &[&bought, &touched, &batch]).await?;
            expect(&answers[0], 1)?;
            expect(&answers[1], DOCTORS)?;
            expect(&answers[2], MIXED_BATCH_ROWS)
        }
        other => Err(Failure::Usage(format!("{other} is not a data pipeline"))),
    }
}

fn database_name(pipeline: &str) -> String {
    format!("tb_perf_{}_{pipeline}", std::process::id())
}

/// Warmup plus measured iterations, one at a time.
struct Iterations<'a> {
    times: &'a StageTimes,
    warmup: usize,
    samples: Vec<Sample>,
}

impl<'a> Iterations<'a> {
    fn new(times: &'a StageTimes, warmup: usize) -> Self {
        Self {
            times,
            warmup,
            samples: Vec::new(),
        }
    }

    /// Start an iteration: drop stage time recorded by setup.
    fn begin(&self) -> Instant {
        self.times.take();
        Instant::now()
    }

    /// End iteration `number`; warmup iterations are not kept.
    fn end(&mut self, number: usize, started: Instant) {
        let total_ns = nanos(started);
        let stages = self.times.take();
        if number >= self.warmup {
            self.samples.push(Sample { total_ns, stages });
        }
    }
}

fn data_pipeline(
    name: &str,
    config: &Config,
    workspace: &Workspace,
    times: &StageTimes,
    runtime: &tokio::runtime::Runtime,
) -> Result<Vec<Sample>> {
    let database = database_name(name);
    let measured = async {
        let options = ConnectionOptions::new(config.address.clone(), database.clone())
            .credentials(config.username.clone(), config.password.clone())
            .http_port(config.http_port);
        let target = Target {
            db: Database::connect(options)
                .await?
                .with_schema(purchase_models::package())?,
            package: purchase_models::package(),
        };
        seed(name, &target, config.warmup + config.iterations).await?;
        let mut iterations = Iterations::new(times, config.warmup);
        for number in 0..config.warmup + config.iterations {
            let started = iterations.begin();
            iteration(name, &target, number).await?;
            iterations.end(number, started);
        }
        Ok::<_, Failure>(iterations.samples)
    };
    // The CLI apply starts its own runtime, so it runs outside `block_on`;
    // the databases are dropped whatever failed.
    let result = workspace
        .apply(config, &database)
        .and_then(|()| runtime.block_on(measured));
    let dropped = runtime.block_on(drop_databases(config, &database));
    let samples = result?;
    dropped?;
    Ok(samples)
}

/// Apply the one-migration chain to a fresh database per iteration, after
/// one warmup apply.
fn migration_pipeline(
    config: &Config,
    workspace: &Workspace,
    times: &StageTimes,
    runtime: &tokio::runtime::Runtime,
) -> Result<Vec<Sample>> {
    let base = database_name("migration_apply");
    let mut created = Vec::new();
    let mut iterations = Iterations::new(times, 1);
    let mut applied = Ok(());
    for number in 0..1 + config.migration_iterations {
        let database = format!("{base}_{number}");
        if let Err(error) = workspace.point_at(config, &database) {
            applied = Err(error);
            break;
        }
        created.push(database);
        let started = iterations.begin();
        if let Err(error) = workspace.cli(&["migration", "apply", "--environment", "perf"]) {
            applied = Err(error);
            break;
        }
        iterations.end(number, started);
    }
    let mut dropped = Ok(());
    for database in &created {
        if let Err(error) = runtime.block_on(drop_databases(config, database)) {
            dropped = Err(error);
        }
    }
    applied?;
    dropped?;
    Ok(iterations.samples)
}

fn report(config: &Config, results: Vec<(&str, usize, Vec<Sample>)>) -> Json {
    let pipelines = results
        .into_iter()
        .map(|(name, operations, samples)| {
            let samples = samples
                .into_iter()
                .map(|sample| json!({"total_ns": sample.total_ns, "stages": sample.stages}))
                .collect::<Vec<_>>();
            json!({"name": name, "operations": operations, "samples": samples})
        })
        .collect::<Vec<_>>();
    json!({
        "format": FORMAT,
        "binding": "rust",
        "warmup": config.warmup,
        "pipelines": pipelines,
    })
}

fn run(config: &Config) -> Result<()> {
    let times = stages::install()
        .map_err(|error| Failure::Usage(format!("cannot install the stage layer: {error}")))?;
    let workspace = Workspace::create(config)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut results = Vec::new();
    for (name, operations) in PIPELINES {
        if !config.selected(name) {
            continue;
        }
        eprintln!("pipeline {name}");
        let samples = if name == "migration_apply" {
            migration_pipeline(config, &workspace, &times, &runtime)?
        } else {
            data_pipeline(name, config, &workspace, &times, &runtime)?
        };
        results.push((name, operations, samples));
    }
    write_report(&config.out, &report(config, results))
}

/// One offline op; `rows` is the prebuilt 1k-row plan `encode_rows_1k` encodes.
fn binding_op(name: &str, package: &SchemaPackage<Shop>, rows: &Built) -> Result<()> {
    match name {
        "build_purchase" => drop(std::hint::black_box(purchase(package, "O0".into())?)),
        "build_rows_1k" => drop(std::hint::black_box(insert_items(
            package,
            "batch-",
            0..BATCH_ROWS as i64,
        )?)),
        _ => drop(std::hint::black_box(rows.canonical_bytes()?)),
    }
    Ok(())
}

/// Time the offline ops with the stage layer installed, as in the bindings'
/// `perf-stages` builds, so every SDK carries the same tracing cost.
fn binding(config: &Config) -> Result<()> {
    let _times = stages::install()
        .map_err(|error| Failure::Usage(format!("cannot install the stage layer: {error}")))?;
    let package = purchase_models::package();
    let rows = insert_items(&package, "batch-", 0..BATCH_ROWS as i64)?;
    let mut ops = Vec::new();
    for name in BINDING_OPS {
        let mut samples = Vec::with_capacity(config.iterations);
        for number in 0..config.warmup + config.iterations {
            let started = Instant::now();
            binding_op(name, &package, &rows)?;
            let elapsed = nanos(started);
            if number >= config.warmup {
                samples.push(elapsed);
            }
        }
        ops.push(json!({"name": name, "samples_ns": samples}));
    }
    let report = json!({
        "format": BINDING_FORMAT,
        "binding": "rust",
        "warmup": config.warmup,
        "ops": ops,
    });
    write_report(&config.out, &report)
}

fn write_report(out: &Path, report: &Json) -> Result<()> {
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(report)
        .map_err(|error| Failure::Answer(format!("cannot encode the report: {error}")))?;
    text.push('\n');
    std::fs::write(out, text)?;
    Ok(())
}

fn main() -> ExitCode {
    let outcome = Config::parse(std::env::args().skip(1)).and_then(|config| {
        if config.binding {
            binding(&config)
        } else {
            run(&config)
        }
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
