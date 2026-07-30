use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, RuntimeProjection};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{GeneratedPackage, RustEmitter};

const POSITIVE: &str = include_str!("rust_acceptance/positive.rs");
const NEGATIVE: &str = include_str!("rust_acceptance/negative.rs");
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        Self::new_for_nonce(nonce)
    }

    fn new_for_nonce(nonce: u128) -> Self {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "type-bridge-schema-codegen-rust-acceptance-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("acceptance stage is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn acceptance_stages_are_unique_when_clock_nonce_repeats() {
    let first = Stage::new_for_nonce(u128::MAX);
    let second = Stage::new_for_nonce(u128::MAX);

    assert_ne!(first.path(), second.path());
}

fn project_from_source(source: &str) -> RuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("rust-acceptance.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let resources = RustEmitter::new().code_resources().unwrap();
    project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &RustEmitter::new().generator_handlers(),
        &resources,
    )
    .unwrap()
}

fn emit_from_source(source: &str) -> GeneratedPackage {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("rust-acceptance.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    RustEmitter::new()
        .emit_with_declared_schema(&project_from_source(source), &declared)
        .unwrap()
}

fn emit() -> GeneratedPackage {
    emit_from_source(include_str!("acceptance/schema.yaml"))
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, bytes) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn write_consumer_with_features(root: &Path, name: &str, source: &str, features: &[&str]) {
    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");
    let feat_str = if features.is_empty() {
        String::new()
    } else {
        format!(", features = {:?}", features)
    };
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false{feat_str} }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    ).unwrap();
    fs::write(root.join("src/main.rs"), source).unwrap();
}

fn write_consumer(root: &Path, name: &str, source: &str) {
    write_consumer_with_features(root, name, source, &["test-harness"]);
}

static CARGO_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cargo(arguments: &[&str], _target: &Path) -> Output {
    let _guard = CARGO_MUTEX.lock().unwrap();
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace_target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("tmp_acceptance_target");
    let target_dir = env::var_os("ACCEPTANCE_TARGET_DIR")
        .unwrap_or_else(|| workspace_target.as_os_str().to_os_string());
    Command::new(executable)
        .args(arguments)
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
        .unwrap()
}

#[test]
fn generated_rust_crate_compiles_rejects_invalid_types_and_runs() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let positive = stage.path().join("positive");
    let negative = stage.path().join("negative");
    write_package(&emit(), &generated);
    write_consumer(&positive, "rust-projection-positive", POSITIVE);
    write_consumer(&negative, "rust-projection-negative", NEGATIVE);

    let positive_manifest = positive.join("Cargo.toml");
    let positive_output = cargo(
        &[
            "run",
            "--quiet",
            "--manifest-path",
            positive_manifest.to_str().unwrap(),
        ],
        &stage.path().join("positive-target"),
    );
    assert!(
        positive_output.status.success(),
        "positive generated consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&positive_output.stdout),
        String::from_utf8_lossy(&positive_output.stderr),
    );

    let negative_manifest = negative.join("Cargo.toml");
    let negative_output = cargo(
        &[
            "check",
            "--quiet",
            "--manifest-path",
            negative_manifest.to_str().unwrap(),
        ],
        &stage.path().join("negative-target"),
    );
    assert!(
        !negative_output.status.success(),
        "negative generated consumer unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&negative_output.stderr);
    assert!(
        stderr.contains("mismatched types"),
        "negative failure was not a type mismatch:\n{stderr}"
    );
    assert!(
        stderr.contains("EventRef"),
        "negative failure omitted the distinct reference type:\n{stderr}"
    );
    assert!(
        stderr.contains("RoleToken"),
        "negative failure omitted the owner-branded role token:\n{stderr}"
    );
    assert!(
        stderr.contains("expected `Binding<AppSchema, Person, _>`")
            && stderr.contains("found `Binding<AppSchema, Event>`"),
        "negative failure omitted the generated reachability endpoint proof:\n{stderr}"
    );
    assert!(
        stderr.contains("SelectedRow requires between 1 and 16 fields"),
        "negative failure omitted the selected-row arity ceiling:\n{stderr}"
    );
    assert!(
        stderr.contains("Output == Event") || stderr.contains("Output = Event"),
        "negative failure omitted the selected-output type proof:\n{stderr}"
    );
    assert!(
        stderr.contains("SingularSelectedShape"),
        "negative failure omitted the collection-terminal boundary:\n{stderr}"
    );
    assert!(
        stderr.contains("cannot move out of `read` because it is borrowed"),
        "negative failure omitted the active-read close boundary:\n{stderr}"
    );
}

#[test]
fn generated_subtype_association_compiles_as_ordinary_dependency() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("ordinary-consumer");
    let pkg = emit_from_source(
        "format: typebridge.schema/v2\nentities:\n  base:\n    abstract: true\n  record: { sub: base }\n  child: { sub: base }\n",
    );
    write_package(&pkg, &generated);
    write_consumer_with_features(
        &consumer,
        "ordinary-consumer",
        "use generated::{Model, Record}; fn main() { let _ = Record::TYPE_ID_JSON; }\n",
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("ordinary-consumer-target"),
    );
    assert!(
        output.status.success(),
        "ordinary consumer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const MANAGER_MATRIX_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  person-only: { value: string }
  company-only: { value: string }
entities:
  helper: {}
  party:
    abstract: true
    owns: { name: { key: true } }
  person:
    sub: party
    owns: { person-only: { card: 1 } }
  company:
    sub: party
    owns: { company-only: { card: 1 } }
  root:
    owns: { name: { key: true } }
  root-child: { sub: root }
  empty: { abstract: true }
  outsider: {}
relations:
  employment:
    abstract: true
    relates: { employee: { card: 1 } }
  placement:
    sub: employment
  contract:
    sub: employment
    relates: { contractor: { as: employee, card: 1 } }
plays:
  helper:
    employment: { employee: { card: 1 } }
    contract: { contractor: { card: 1 } }
"#;

#[test]
fn generated_external_manager_contract_compiles_exact_matrix() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("manager-consumer");
    let package = emit_from_source(MANAGER_MATRIX_SCHEMA);
    write_package(&package, &generated);
    write_consumer_with_features(
        &consumer,
        "manager-consumer",
        r#"
use generated::{AppSchema, Contract, ContractCreate, ContractType, Empty, Employment, EmploymentFamily, EmploymentType, Helper, Model, Name, Never, Party, PartyFamily, PartyType, Person, PersonCreate, PersonType, Placement, Root, RootChild, RootFamily};
use type_bridge::{Database, EntityManager, EntitySubtypeManager, Predicate, RelationManager, RelationSubtypeManager, WriteTransaction};
use type_bridge::value::Text;
async fn positive(db: &Database<AppSchema>, create: PersonCreate) {
    let exact = db.entities::<Person>();
    let _: EntityManager<'_, AppSchema, Person> = exact;
    let _: Person = exact.insert(create.clone()).await.unwrap();
    let _: Person = exact.put(create.clone()).await.unwrap();
    let _: Person = exact.update("0x1", create.clone()).await.unwrap();
    let _: Vec<Person> = exact.insert_many(vec![create.clone()]).await.unwrap();
    let _: Vec<Person> = exact.put_many(vec![create.clone()]).await.unwrap();
    let _: () = exact.delete("0x1").await.unwrap();
    let _: Option<Person> = exact.get_by_iid("0x1").await.unwrap();
    let _: Vec<Person> = exact.all().await.unwrap();
    let _: u64 = exact.count().await.unwrap();
    let _: EntitySubtypeManager<'_, AppSchema, Person> = exact.subtypes();
    let _: Option<Person> = exact.subtypes().get_by_iid("0x1").await.unwrap();
    let _: Vec<Person> = exact.subtypes().all().await.unwrap();
    let _: u64 = exact.subtypes().count().await.unwrap();
    let family = db.entities::<Party>().subtypes();
    let family_value: Option<PartyFamily> = family.get_by_iid("0x1").await.unwrap();
    if let Some(value) = family_value { let _ = value.name(); narrow(value); }
    let _: Option<PartyFamily> = family.get_by_iid("0x1").await.unwrap();
    let _: Vec<PartyFamily> = family.all().await.unwrap();
    let _: u64 = family.count().await.unwrap();
    let root_value: Option<RootFamily> = db.entities::<Root>().subtypes().get_by_iid("0x1").await.unwrap();
    if let Some(value) = root_value { match value { RootFamily::Root(root) => { let _ = root.name(); }, RootFamily::RootChild(child) => { let _ = child.name(); } } }
    let root_values: Vec<RootFamily> = db.entities::<Root>().subtypes().all().await.unwrap();
    for value in root_values { match value { RootFamily::Root(root) => { let _ = root.name(); }, RootFamily::RootChild(child) => { let _ = child.name(); } } }
    let _: Option<RootChild> = db.entities::<RootChild>().subtypes().get_by_iid("0x1").await.unwrap();
    let _: Option<Never> = db.entities::<Empty>().subtypes().get_by_iid("0x1").await.unwrap();
    let _: Vec<Never> = db.entities::<Empty>().subtypes().all().await.unwrap();
}

fn narrow(value: PartyFamily) { match value { PartyFamily::Person(person) => { let _ = person.name(); let _ = person.person_only(); }, PartyFamily::Company(company) => { let _ = company.name(); let _ = company.company_only(); } } }

async fn relation_positive(db: &Database<AppSchema>, create: ContractCreate) {
    let exact = db.relations::<Contract>();
    let _: RelationManager<'_, AppSchema, Contract> = exact;
    let _: Contract = exact.insert(create.clone()).await.unwrap();
    let _: Contract = exact.put(create.clone()).await.unwrap();
    let _: Contract = exact.update("0x1", create.clone()).await.unwrap();
    let _: Vec<Contract> = exact.insert_many(vec![create.clone()]).await.unwrap();
    let _: Vec<Contract> = exact.put_many(vec![create.clone()]).await.unwrap();
    let _: () = exact.delete("0x1").await.unwrap();
    let _: Option<Contract> = exact.get_by_iid("0x1").await.unwrap();
    let _: Vec<Contract> = exact.all().await.unwrap();
    let _: u64 = exact.count().await.unwrap();
    let _: RelationSubtypeManager<'_, AppSchema, Contract> = exact.subtypes();
    let _: Option<Contract> = exact.subtypes().get_by_iid("0x1").await.unwrap();
    let _: Vec<Contract> = exact.subtypes().all().await.unwrap();
    let _: u64 = exact.subtypes().count().await.unwrap();
    let family = db.relations::<Employment>().subtypes();
    let _: Option<EmploymentFamily> = family.get_by_iid("0x1").await.unwrap();
    let _: Vec<EmploymentFamily> = family.all().await.unwrap();
    let _: u64 = family.count().await.unwrap();
}

async fn transaction_positive(db: &Database<AppSchema>, create: PersonCreate, relation_create: ContractCreate) {
    let tx: WriteTransaction<'_, AppSchema> = db.write().await.unwrap();
    let _: Person = tx.entities::<Person>().insert(create.clone()).await.unwrap();
    let _: Person = tx.entities::<Person>().put(create.clone()).await.unwrap();
    let _: Person = tx.entities::<Person>().update("0x1", create.clone()).await.unwrap();
    let _: Vec<Person> = tx.entities::<Person>().insert_many(vec![create.clone()]).await.unwrap();
    let _: Vec<Person> = tx.entities::<Person>().put_many(vec![create.clone()]).await.unwrap();
    let _: () = tx.entities::<Person>().delete("0x1").await.unwrap();
    let _: Option<Person> = tx.entities::<Person>().get_by_iid("0x1").await.unwrap();
    let _: Vec<Person> = tx.entities::<Person>().all().await.unwrap();
    let _: u64 = tx.entities::<Person>().count().await.unwrap();
    let _: Contract = tx.relations::<Contract>().insert(relation_create.clone()).await.unwrap();
    let _: Contract = tx.relations::<Contract>().put(relation_create.clone()).await.unwrap();
    let _: Contract = tx.relations::<Contract>().update("0x1", relation_create.clone()).await.unwrap();
    let _: Vec<Contract> = tx.relations::<Contract>().insert_many(vec![relation_create.clone()]).await.unwrap();
    let _: Vec<Contract> = tx.relations::<Contract>().put_many(vec![relation_create.clone()]).await.unwrap();
    let _: () = tx.relations::<Contract>().delete("0x1").await.unwrap();
    let _: Option<Contract> = tx.relations::<Contract>().get_by_iid("0x1").await.unwrap();
    let _: Vec<Contract> = tx.relations::<Contract>().all().await.unwrap();
    let _: u64 = tx.relations::<Contract>().count().await.unwrap();
    tx.commit().await.unwrap();
    let tx = db.write().await.unwrap();
    tx.rollback().await.unwrap();
    let _dropped = db.write().await.unwrap();
}

fn query_positive(db: &Database<AppSchema>) {
    let mut session = db.query().unwrap();
    let party = session.subtypes::<Party>().unwrap();
    let person = session.exact::<Person>().unwrap();
    let helper = session.exact::<Helper>().unwrap();
    let placement = session.exact::<Placement>().unwrap();
    let contract = session.exact::<Contract>().unwrap();
    let party_name = party.field(PartyType::name);
    let person_name = person.field(PartyType::name);
    let person_only = person.field(PersonType::person_only);
    let placement_employee = placement.role(EmploymentType::employee);
    let contract_contractor = contract.role(ContractType::contractor);
    let predicate: Predicate<AppSchema> = party_name.eq(Name::new("Alice").unwrap())
        & party_name.starts_with(Text::new("Al").unwrap())
        & person_only.eq(Text::new("x").unwrap())
        & person_name.eq_field(party_name)
        & placement_employee.connects(helper)
        & contract_contractor.connects(helper);
    let _ = predicate;
    let _party_query = session.query(party).unwrap();
    let _person_query = session.query(person).unwrap();
}
fn main() {}
"#,
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--offline",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("manager-target"),
    );
    let manifest = fs::read_to_string(consumer.join("Cargo.toml")).unwrap();
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("[patch.crates-io]")
        .next()
        .unwrap();
    let entries = deps
        .lines()
        .map(str::trim)
        .filter(|line| line.contains('=') && !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|line| line.starts_with("generated =")));
    assert!(
        entries
            .iter()
            .any(|line| line.starts_with("type-bridge =")
                && line.contains("default-features = false"))
    );
    for forbidden in [
        "type-bridge-orm",
        "type-bridge-contract",
        "type-bridge-schema",
        "type-bridge-query",
        "type-bridge-schema-codegen",
        "provider",
    ] {
        assert!(!entries.iter().any(|line| line.contains(forbidden)));
    }
    assert!(
        output.status.success(),
        "generated manager consumer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_external_manager_negative_matrix_has_specific_diagnostics() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("negative-manager-consumer");
    write_package(
        &emit_from_source("format: typebridge.schema/v2\nentities:\n  party: { abstract: true }\n"),
        &generated,
    );
    write_consumer_with_features(
        &consumer,
        "negative-manager-consumer",
        r#"use generated::{AppSchema, Party}; use type_bridge::Database;
async fn insert(db: &Database<AppSchema>) { let _ = db.entities::<Party>().insert(todo!()); }
async fn insert_many(db: &Database<AppSchema>) { let _ = db.entities::<Party>().insert_many(vec![]); }
async fn put(db: &Database<AppSchema>) { let _ = db.entities::<Party>().put(todo!()); }
async fn put_many(db: &Database<AppSchema>) { let _ = db.entities::<Party>().put_many(vec![]); }
async fn update(db: &Database<AppSchema>) { let _ = db.entities::<Party>().update("0x1", todo!()); }
async fn delete(db: &Database<AppSchema>) { let _ = db.entities::<Party>().delete("0x1"); }
async fn get_by_iid(db: &Database<AppSchema>) { let _ = db.entities::<Party>().get_by_iid("0x1"); }
async fn all(db: &Database<AppSchema>) { let _ = db.entities::<Party>().all(); }
async fn count(db: &Database<AppSchema>) { let _ = db.entities::<Party>().count(); }
fn main() {}"#,
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--offline",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("negative-manager-target"),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for method in [
        "insert",
        "insert_many",
        "put",
        "put_many",
        "update",
        "delete",
        "get_by_iid",
        "all",
        "count",
    ] {
        assert!(stderr.contains(&format!("method `{method}`")));
    }
    assert!(stderr.matches("error[E0599]").count() >= 9);
    assert!(stderr.matches("CompleteModel").count() >= 9);
    assert!(
        stderr.contains("CompleteModel")
            && !stderr.contains("unresolved import")
            && !stderr.contains("cannot find")
    );
}

#[test]
fn generated_external_manager_remaining_negative_boundaries() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let package = emit_from_source(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  person-only: { value: string }
entities:
  helper: {}
  party: { abstract: true }
  person: { sub: party, owns: { name: { key: true }, person-only: { card: 1 } } }
  company: { sub: party, owns: { name: { key: true } } }
  outsider: {}
relations:
  employment: { relates: { employee: { card: 1 } } }
  contract:
    sub: employment
    relates: { contractor: { as: employee, card: 1 } }
  pact: { abstract: true, relates: { side: { card: 1 } } }
plays:
  helper:
    employment: { employee: { card: 1 } }
    contract: { contractor: { card: 1 } }
    pact: { side: { card: 1 } }
"#,
    );
    write_package(&package, &generated);
    let cases = [
        (
            "subtype-write",
            "use generated::{AppSchema, Person}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.entities::<Person>().subtypes().insert(todo!()); } fn main() {}",
            "no method named `insert`",
        ),
        (
            "wrong-create",
            "use generated::{AppSchema, Person, OutsiderCreate}; use type_bridge::Database; async fn f(db: &Database<AppSchema>, x: OutsiderCreate) { let _ = db.entities::<Person>().insert(x); } fn main() {}",
            "mismatched types",
        ),
        (
            "relation-entity",
            "use generated::{AppSchema, Employment}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.entities::<Employment>(); } fn main() {}",
            "EntityModel",
        ),
        (
            "reference-entity",
            "use generated::{AppSchema, PersonRef}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.entities::<PersonRef>(); } fn main() {}",
            "EntityModel",
        ),
        (
            "child-before-narrow",
            "use generated::{AppSchema, Party, PartyFamily}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let x: PartyFamily = db.entities::<Party>().subtypes().all().await.unwrap().remove(0); let _ = x.person_only(); } fn main() {}",
            "no method named `person_only`",
        ),
        (
            "entity-relation-manager",
            "use generated::{AppSchema, Person}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.relations::<Person>(); } fn main() {}",
            "RelationModel",
        ),
        (
            "reference-relation-manager",
            "use generated::{AppSchema, EmploymentRef}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.relations::<EmploymentRef>(); } fn main() {}",
            "RelationModel",
        ),
        (
            "relation-subtype-write",
            "use generated::{AppSchema, Employment}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.relations::<Employment>().subtypes().insert(todo!()); } fn main() {}",
            "no method named `insert`",
        ),
        (
            "abstract-relation-write",
            "use generated::{AppSchema, Pact}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let _ = db.relations::<Pact>().insert(todo!()); } fn main() {}",
            "CompleteModel",
        ),
        (
            "transaction-commit-while-borrowed",
            "use generated::{AppSchema, Person}; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let tx = db.write().await.unwrap(); let manager = tx.entities::<Person>(); tx.commit().await.unwrap(); let _ = manager.count().await.unwrap(); } fn main() {}",
            "cannot move out of `tx` because it is borrowed",
        ),
        (
            "transaction-not-cloneable",
            "use generated::AppSchema; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let tx = db.write().await.unwrap(); let _second = tx.clone(); } fn main() {}",
            "no method named `clone`",
        ),
        (
            "query-cross-owner-field",
            "use generated::{AppSchema, Party, PersonType}; use type_bridge::Database; fn f(db: &Database<AppSchema>) { let mut session = db.query().unwrap(); let party = session.subtypes::<Party>().unwrap(); let _ = party.field(PersonType::person_only); } fn main() {}",
            "mismatched types",
        ),
        (
            "query-ordered-operator-on-text-field",
            "use generated::{AppSchema, Person, PersonType}; use type_bridge::Database; fn f(db: &Database<AppSchema>) { let mut session = db.query().unwrap(); let person = session.exact::<Person>().unwrap(); let _ = person.field(PersonType::name).gt(2_i64); } fn main() {}",
            "OrderedValued",
        ),
        (
            "query-specialized-away-ancestor-role",
            "use generated::{AppSchema, Contract, EmploymentType}; use type_bridge::Database; fn f(db: &Database<AppSchema>) { let mut session = db.query().unwrap(); let contract = session.exact::<Contract>().unwrap(); let _ = contract.role(EmploymentType::employee); } fn main() {}",
            "mismatched types",
        ),
        (
            "query-role-rejects-non-player",
            "use generated::{AppSchema, Contract, ContractType, Person}; use type_bridge::Database; fn f(db: &Database<AppSchema>) { let mut session = db.query().unwrap(); let contract = session.exact::<Contract>().unwrap(); let person = session.exact::<Person>().unwrap(); let _ = contract.role(ContractType::contractor).connects(person); } fn main() {}",
            "mismatched types",
        ),
        (
            "query-equality-rejects-wrong-domain",
            "use generated::{AppSchema, Person, PersonType}; use type_bridge::Database; fn f(db: &Database<AppSchema>) { let mut session = db.query().unwrap(); let person = session.exact::<Person>().unwrap(); let _ = person.field(PersonType::name).eq(2_i64); } fn main() {}",
            "type mismatch",
        ),
        (
            "transaction-double-terminal",
            "use generated::AppSchema; use type_bridge::Database; async fn f(db: &Database<AppSchema>) { let tx = db.write().await.unwrap(); tx.commit().await.unwrap(); tx.rollback().await.unwrap(); } fn main() {}",
            "use of moved value: `tx`",
        ),
    ];
    for (name, source, reason) in cases {
        let consumer = stage.path().join(name);
        write_consumer_with_features(&consumer, name, source, &[]);
        let output = cargo(
            &[
                "check",
                "--offline",
                "--manifest-path",
                consumer.join("Cargo.toml").to_str().unwrap(),
            ],
            &stage.path().join(format!("{name}-target")),
        );
        assert!(!output.status.success(), "{name} unexpectedly compiled");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(reason)
                && !stderr.contains("unresolved import")
                && !stderr.contains("cannot find"),
            "{name}: {stderr}"
        );
    }
}

#[test]
fn generated_external_cross_schema_and_forged_capability_boundaries() {
    let stage = Stage::new();
    let generated_a = stage.path().join("generated-a");
    let generated_b = stage.path().join("generated-b");
    let consumer = stage.path().join("cross-boundary-consumer");
    const CROSS_SCHEMA: &str = "format: typebridge.schema/v2\nentities:\n  person: {}\nrelations:\n  pact: { relates: { side: { card: 1 } } }\nplays:\n  person:\n    pact: [side]\n";
    let package_a = emit_from_source(CROSS_SCHEMA);
    let package_b = emit_from_source(CROSS_SCHEMA);
    write_package(&package_a, &generated_a);
    write_package(&package_b, &generated_b);
    let manifest_b = generated_b.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest_b).unwrap();
    text = text.replace(
        "name = \"type-bridge-generated-schema\"",
        "name = \"schema-b\"",
    );
    fs::write(manifest_b, text).unwrap();
    fs::create_dir_all(consumer.join("src")).unwrap();
    fs::write(consumer.join("src/main.rs"), "use schema_a::AppSchema; use schema_b::{Pact, Person}; use type_bridge::Database; fn f(db_a: &Database<AppSchema>) { let _ = db_a.entities::<Person>(); } fn g(db_a: &Database<AppSchema>) { let _ = db_a.relations::<Pact>(); } fn main() {}").unwrap();
    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");
    fs::write(consumer.join("Cargo.toml"), format!("[package]\nname=\"cross-boundary-consumer\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[dependencies]\nschema_a={{package=\"type-bridge-generated-schema\",path=\"../generated-a\"}}\nschema_b={{package=\"schema-b\",path=\"../generated-b\"}}\ntype-bridge={{path=\"{rust_path}\",default-features=false}}\n[patch.crates-io]\ntype-bridge={{path=\"{rust_path}\"}}\n[workspace]\n")).unwrap();
    let output = cargo(
        &[
            "check",
            "--offline",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("cross-boundary-target"),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("EntityModel") && stderr.contains("AppSchema"),
        "cross-schema entity boundary did not fail on its model brand:\n{stderr}"
    );
    assert!(
        stderr.contains("RelationModel"),
        "cross-schema relation boundary did not fail on its model brand:\n{stderr}"
    );
    for bad in [
        "failed to get",
        "failed to load",
        "No such file",
        "unresolved import",
        "cannot find",
    ] {
        assert!(!stderr.contains(bad), "invalid staging failure: {stderr}");
    }

    let forged = stage.path().join("forged-capability-consumer");
    write_package(&package_a, &stage.path().join("generated"));
    write_consumer_with_features(
        &forged,
        "forged-capability-consumer",
        "use generated::{HydratedRow, MaterializeModel, Person}; use type_bridge::__codegen::HydrationCapability; fn forge(row: &HydratedRow) { let _ = Person::materialize(row, &HydrationCapability::new()); } fn main() {}",
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--offline",
            "--manifest-path",
            forged.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("forged-capability-target"),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("HydrationCapability")
            && stderr.contains("new")
            && (stderr.contains("private") || stderr.contains("E0624"))
    );
    for bad in [
        "failed to get",
        "failed to load",
        "No such file",
        "missing manifest",
        "unresolved import",
        "cannot find",
    ] {
        assert!(!stderr.contains(bad), "forge staging failure: {stderr}");
    }
    assert!(stderr.contains("HydrationCapability") && stderr.contains("new"));
    for bad in [
        "failed to get",
        "failed to load",
        "No such file",
        "unresolved import",
        "cannot find",
    ] {
        assert!(!stderr.contains(bad));
    }
}

#[test]
fn generated_declaration_boundary_matrix_is_scoped() {
    let package = emit_from_source(MANAGER_MATRIX_SCHEMA);
    let declarations =
        String::from_utf8(package.get("src/declaration.rs").unwrap().to_vec()).unwrap();
    let read = String::from_utf8(package.get("src/read.rs").unwrap().to_vec()).unwrap();
    let manifest = String::from_utf8(package.get("Cargo.toml").unwrap().to_vec()).unwrap();
    fn complete_line<'a>(src: &'a str, name: &str) -> &'a str {
        src.lines()
            .find(|line| {
                line.starts_with(&format!("impl CompleteModel for {name} "))
                    .to_owned()
            })
            .unwrap()
    }
    fn subtype_block<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("impl SubtypeRootModel for {name} "))
            .unwrap();
        let end = start + src[start..].find(" } }\n").unwrap() + 4;
        &src[start..end]
    }
    fn enum_body<'a>(src: &'a str, name: &str) -> Vec<&'a str> {
        let start = src.find(&format!("pub enum {name} {{")).unwrap();
        let body = &src[start + src[start..].find('{').unwrap() + 1..];
        body[..body.find("\n}\n").unwrap()]
            .lines()
            .filter_map(|l| l.trim().strip_suffix(','))
            .map(|l| l.split('(').next().unwrap())
            .collect()
    }
    for name in [
        "Helper",
        "Person",
        "Company",
        "Root",
        "RootChild",
        "Outsider",
        "Contract",
        "Placement",
    ] {
        let expected = format!(
            "impl CompleteModel for {name} {{ type Create = crate::create::{name}Create; fn iid(&self) -> &str {{ self.iid() }} }}"
        );
        assert_eq!(complete_line(&declarations, name), expected);
    }
    for name in ["Party", "Empty", "Employment"] {
        assert!(
            !declarations
                .lines()
                .any(|line| line.starts_with(&format!("impl CompleteModel for {name} ")))
        );
    }
    let leaf_expected = |name: &str| {
        format!(
            "impl SubtypeRootModel for {name} {{ type Subtypes = {name}; fn __tb_dispatch_subtype(__tb_row: &HydratedRow, __tb_cap: &HydrationCapability) -> Result<Self::Subtypes, ValidationError> {{\nif __tb_row.type_id_json() == {name}::TYPE_ID_JSON {{ {name}::materialize(__tb_row, __tb_cap) }} else {{ Err(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\")) }} }} }}"
        )
    };
    let family_expected = |root: &str, family: &str, arms: &[&str]| {
        let mut value = format!(
            "impl SubtypeRootModel for {root} {{ type Subtypes = {family}; fn __tb_dispatch_subtype(__tb_row: &HydratedRow, __tb_cap: &HydrationCapability) -> Result<Self::Subtypes, ValidationError> {{\nmatch __tb_row.type_id_json() {{\n"
        );
        for arm in arms {
            value.push_str(&format!("{arm}::TYPE_ID_JSON => {arm}::materialize(__tb_row, __tb_cap).map({family}::{arm}),\n"));
        }
        value.push_str(
            "_ => Err(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\")),\n} } }",
        );
        value
    };
    let never_expected = |name: &str| {
        format!(
            "impl SubtypeRootModel for {name} {{ type Subtypes = runtime::Never; fn __tb_dispatch_subtype(__tb_row: &HydratedRow, __tb_cap: &HydrationCapability) -> Result<Self::Subtypes, ValidationError> {{\nErr(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\")) }} }}"
        )
    };
    assert_eq!(
        subtype_block(&declarations, "Person"),
        leaf_expected("Person")
    );
    assert_eq!(
        subtype_block(&declarations, "Party"),
        family_expected("Party", "PartyFamily", &["Company", "Person"])
    );
    assert_eq!(
        subtype_block(&declarations, "Root"),
        family_expected("Root", "RootFamily", &["Root", "RootChild"])
    );
    assert_eq!(
        subtype_block(&declarations, "Empty"),
        never_expected("Empty")
    );
    assert_eq!(
        subtype_block(&declarations, "Employment"),
        family_expected("Employment", "EmploymentFamily", &["Contract", "Placement"])
    );
    assert_eq!(
        subtype_block(&declarations, "Contract"),
        leaf_expected("Contract")
    );
    assert_eq!(enum_body(&read, "PartyFamily"), vec!["Company", "Person"]);
    assert_eq!(enum_body(&read, "RootFamily"), vec!["Root", "RootChild"]);
    assert_eq!(
        enum_body(&read, "EmploymentFamily"),
        vec!["Contract", "Placement"]
    );
    let import = declarations
        .lines()
        .find(|l| l.starts_with("use crate::runtime::{"))
        .unwrap();
    assert_eq!(
        import,
        "use crate::runtime::{self, AbstractModel, CompleteModel, EntityModel, HydratedRow, HydrationCapability, MaterializeModel, Model, ModelFamily, NominalUpcast, ReferenceModel, RelationModel, RoleTokenCompatible, RoleUpcast, SubtypeRootModel, ThingModel, ValidationError};"
    );
    assert!(
        declarations.contains("__tb_dispatch_subtype")
            && !declarations.contains("fn dispatch_subtype")
    );
    let client_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust/src");
    let lib_source = fs::read_to_string(client_src.join("lib.rs")).unwrap();
    assert!(
        lib_source.contains("RelationManager") && lib_source.contains("RelationSubtypeManager"),
        "client crate no longer exports the relation manager surface"
    );
    let session_source = fs::read_to_string(client_src.join("session.rs")).unwrap();
    assert!(
        session_source.contains("pub fn relations<M>"),
        "client session no longer exposes Database::relations"
    );
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("[features]")
        .next()
        .unwrap();
    let dep_lines = deps
        .lines()
        .filter(|line| line.contains('='))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let bridge = dep_lines
        .iter()
        .filter(|line| line.starts_with("type-bridge"))
        .collect::<Vec<_>>();
    assert_eq!(bridge.len(), 1);
    assert!(
        bridge[0].starts_with("type-bridge =") && bridge[0].contains("default-features = false")
    );
    for forbidden in [
        "type-bridge-orm",
        "type-bridge-contract",
        "type-bridge-schema",
        "type-bridge-query",
        "type-bridge-schema-codegen",
    ] {
        assert!(!dep_lines.iter().any(|line| line.starts_with(forbidden)));
    }
}

#[test]
fn generated_subtype_closures_cover_entity_and_relation_roots() {
    let source = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  concrete-root:
    owns: { name: { key: true } }
  concrete-child:
    sub: concrete-root
  abstract-root:
    abstract: true
  abstract-mid:
    sub: abstract-root
    abstract: true
  abstract-leaf-a: { sub: abstract-mid }
  abstract-leaf-b: { sub: abstract-root }
  single-abstract:
    abstract: true
  single-child: { sub: single-abstract }
  empty-abstract: { abstract: true }
  concrete-leaf: {}
  outsider: {}
  helper: {}
relations:
  concrete-relation:
    owns: { name: { card: 1 } }
    relates: { member: { card: 1 } }
  concrete-relation-child:
    sub: concrete-relation
    owns: { name: { card: 1 } }
    relates: { child-member: { as: member, card: 1 } }
  abstract-relation:
    abstract: true
    owns: { name: { card: 1 } }
    relates: { member: { card: 1 } }
  abstract-relation-mid:
    sub: abstract-relation
    abstract: true
    owns: { name: { card: 1 } }
    relates: { mid-member: { as: member, card: 1 } }
  abstract-relation-leaf:
    sub: abstract-relation-mid
  abstract-relation-leaf-b: { sub: abstract-relation }
  single-abstract-relation:
    abstract: true
    relates: { member: { card: 1 } }
  single-relation-child:
    sub: single-abstract-relation
  empty-abstract-relation: { abstract: true }
  relation-leaf: {}
  relation-outsider: {}
plays:
  helper:
    concrete-relation: { member: { card: 1 } }
    concrete-relation-child: { child-member: { card: 1 } }
    abstract-relation: { member: { card: 1 } }
    abstract-relation-mid: { mid-member: { card: 1 } }
    single-abstract-relation: { member: { card: 1 } }
"#;
    let package = emit_from_source(source);
    let declarations =
        String::from_utf8(package.get("src/declaration.rs").unwrap().to_vec()).unwrap();
    let read = String::from_utf8(package.get("src/read.rs").unwrap().to_vec()).unwrap();
    fn enum_body<'a>(read: &'a str, name: &str) -> &'a str {
        let start = read.find(&format!("pub enum {name} {{")).unwrap();
        let body_start = start + read[start..].find('{').unwrap() + 1;
        let end = body_start + read[body_start..].find("\n}\n").unwrap();
        &read[body_start..end]
    }
    fn variant_names(body: &str) -> Vec<&str> {
        body.lines()
            .filter_map(|line| line.trim().strip_suffix(','))
            .map(|line| line.split('(').next().unwrap())
            .collect()
    }
    assert!(read.contains("pub enum ConcreteRootFamily"));
    assert!(read.contains("ConcreteRoot(ConcreteRoot),\n  ConcreteChild(ConcreteChild)"));
    assert!(read.contains("pub enum AbstractRootFamily"));
    assert!(read.contains("AbstractLeafB(AbstractLeafB),\n  AbstractLeafA(AbstractLeafA)"));
    assert!(read.contains("pub enum SingleAbstractFamily"));
    assert!(read.contains("SingleChild(SingleChild)"));
    assert!(read.contains("pub enum ConcreteRelationFamily"));
    assert!(read.contains(
        "ConcreteRelation(ConcreteRelation),\n  ConcreteRelationChild(ConcreteRelationChild)"
    ));
    assert!(read.contains("pub enum AbstractRelationFamily"));
    assert!(read.contains("AbstractRelationLeafB(AbstractRelationLeafB),\n  AbstractRelationLeaf(AbstractRelationLeaf)"));
    assert_eq!(
        variant_names(enum_body(&read, "ConcreteRootFamily")),
        vec!["ConcreteRoot", "ConcreteChild"]
    );
    assert_eq!(
        variant_names(enum_body(&read, "AbstractRootFamily")),
        vec!["AbstractLeafB", "AbstractLeafA"]
    );
    assert_eq!(
        variant_names(enum_body(&read, "SingleAbstractFamily")),
        vec!["SingleChild"]
    );
    assert_eq!(
        variant_names(enum_body(&read, "ConcreteRelationFamily")),
        vec!["ConcreteRelation", "ConcreteRelationChild"]
    );
    assert_eq!(
        variant_names(enum_body(&read, "AbstractRelationFamily")),
        vec!["AbstractRelationLeafB", "AbstractRelationLeaf"]
    );
    assert!(!enum_body(&read, "ConcreteRootFamily").contains("Outsider"));
    assert!(!enum_body(&read, "AbstractRelationFamily").contains("RelationOutsider"));
    for (family, members) in [
        ("ConcreteRootFamily", vec!["ConcreteRoot", "ConcreteChild"]),
        ("AbstractRootFamily", vec!["AbstractLeafA", "AbstractLeafB"]),
        (
            "ConcreteRelationFamily",
            vec!["ConcreteRelation", "ConcreteRelationChild"],
        ),
        (
            "AbstractRelationFamily",
            vec!["AbstractRelationLeaf", "AbstractRelationLeafB"],
        ),
    ] {
        let model_start = declarations.find("pub const MODEL_DECLARATIONS").unwrap();
        let model_end = model_start + declarations[model_start..].find("];\n").unwrap() + 2;
        let model_body = &declarations[model_start..model_end];
        let shell_order = model_body
            .split("target_name: \"")
            .skip(1)
            .filter_map(|part| part.split('"').next())
            .filter(|name| members.contains(name))
            .collect::<Vec<_>>();
        assert_eq!(
            variant_names(enum_body(&read, family)),
            shell_order,
            "family {family}"
        );
    }
    assert_eq!(
        variant_names(enum_body(&read, "SingleAbstractRelationFamily")),
        vec!["SingleRelationChild"]
    );
    assert!(!read.contains("EmptyAbstractRelationFamily"));
    for absent in [
        "EmptyAbstractFamily",
        "EmptyAbstractRelationFamily",
        "ConcreteLeafFamily",
        "RelationLeafFamily",
    ] {
        assert!(!read.contains(&format!("pub enum {absent}")));
    }
    for forbidden in [
        "AbstractMid(",
        "AbstractRoot(AbstractRoot)",
        "Outsider(Outsider)",
    ] {
        assert!(!read.contains(forbidden));
    }
    for family in [
        "ConcreteRootFamily",
        "AbstractRootFamily",
        "SingleAbstractFamily",
        "ConcreteRelationFamily",
        "AbstractRelationFamily",
        "SingleAbstractRelationFamily",
    ] {
        let body = enum_body(&read, family);
        assert!(!body.contains("Outsider") && !body.contains("RelationOutsider"));
    }
    for (family, absent) in [
        (
            "ConcreteRootFamily",
            ["AbstractRoot", "AbstractMid", "Outsider"],
        ),
        (
            "AbstractRootFamily",
            ["AbstractRoot", "AbstractMid", "Outsider"],
        ),
        (
            "ConcreteRelationFamily",
            [
                "AbstractRelation",
                "AbstractRelationMid",
                "RelationOutsider",
            ],
        ),
        (
            "AbstractRelationFamily",
            [
                "AbstractRelation",
                "AbstractRelationMid",
                "RelationOutsider",
            ],
        ),
    ] {
        let body = enum_body(&read, family);
        for name in absent {
            assert!(!body.contains(&format!("{name}(")));
        }
    }
    for (root, expected) in [
        ("ConcreteRoot", "ConcreteRootFamily"),
        ("AbstractRoot", "AbstractRootFamily"),
        ("SingleAbstract", "SingleAbstractFamily"),
        ("EmptyAbstract", "runtime::Never"),
        ("ConcreteLeaf", "ConcreteLeaf"),
        ("ConcreteRelation", "ConcreteRelationFamily"),
        ("AbstractRelation", "AbstractRelationFamily"),
        ("SingleAbstractRelation", "SingleAbstractRelationFamily"),
        ("EmptyAbstractRelation", "runtime::Never"),
        ("RelationLeaf", "RelationLeaf"),
    ] {
        assert!(declarations.contains(&format!("impl SubtypeRootModel for {root}")));
        assert!(declarations.contains(&format!("type Subtypes = {expected}")));
    }
    let empty = declarations
        .find("impl SubtypeRootModel for EmptyAbstract")
        .unwrap();
    let empty_impl =
        &declarations[empty..empty + declarations[empty..].find(" } }\n").unwrap() + 4];
    assert!(
        empty_impl
            .contains("Err(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\"))")
    );
    let empty_relation = declarations
        .find("impl SubtypeRootModel for EmptyAbstractRelation")
        .unwrap();
    let empty_relation_impl = &declarations[empty_relation
        ..empty_relation + declarations[empty_relation..].find(" } }\n").unwrap() + 4];
    assert!(empty_relation_impl.contains("type Subtypes = runtime::Never"));
    assert!(
        empty_relation_impl
            .contains("Err(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\"))")
    );
    let client_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust/src");
    let lib_source = fs::read_to_string(client_src.join("lib.rs")).unwrap();
    assert!(
        lib_source.contains("RelationManager") && lib_source.contains("RelationSubtypeManager"),
        "client crate no longer exports the relation manager surface"
    );
    let session_source = fs::read_to_string(client_src.join("session.rs")).unwrap();
    assert!(
        session_source.contains("pub fn relations<M>"),
        "client session no longer exposes Database::relations"
    );
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("closure-consumer");
    write_package(&package, &generated);
    write_consumer_with_features(
        &consumer,
        "closure-consumer",
        r#"
use generated::runtime::{Never, SubtypeRootModel};
use generated::{AbstractLeafA, AbstractLeafB, AbstractRoot, AbstractRootFamily, ConcreteChild, ConcreteLeaf, ConcreteRoot, ConcreteRootFamily, ConcreteRelation, ConcreteRelationChild, ConcreteRelationFamily, AbstractRelation, AbstractRelationFamily, EmptyAbstract, EmptyAbstractRelation, Model, RelationLeaf, SingleAbstract, SingleAbstractFamily, SingleAbstractRelation, SingleAbstractRelationFamily, SingleChild};
fn assert_assoc<Root, Expected>() where Root: SubtypeRootModel<Subtypes = Expected> {}
fn main() {
    assert_assoc::<ConcreteRoot, ConcreteRootFamily>();
    assert_assoc::<AbstractRoot, AbstractRootFamily>();
    assert_assoc::<SingleAbstract, SingleAbstractFamily>();
    assert_assoc::<EmptyAbstract, Never>();
    assert_assoc::<ConcreteLeaf, ConcreteLeaf>();
    assert_assoc::<ConcreteRelation, ConcreteRelationFamily>();
    assert_assoc::<AbstractRelation, AbstractRelationFamily>();
    assert_assoc::<SingleAbstractRelation, SingleAbstractRelationFamily>();
    assert_assoc::<EmptyAbstractRelation, Never>();
    assert_assoc::<RelationLeaf, RelationLeaf>();
    let _ = (AbstractLeafA::TYPE_ID_JSON, AbstractLeafB::TYPE_ID_JSON, ConcreteChild::TYPE_ID_JSON, EmptyAbstractRelation::TYPE_ID_JSON, SingleChild::TYPE_ID_JSON);
}
"#,
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("closure-target"),
    );
    assert!(
        output.status.success(),
        "closure consumer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn two_independently_generated_schemas_cannot_mix_type_branded_handles_or_tokens() {
    let stage = Stage::new();
    let gen_a = stage.path().join("generated_a");
    let gen_b = stage.path().join("generated_b");
    let cross_neg = stage.path().join("cross_schema_negative");

    let pkg_a = emit_from_source("format: typebridge.schema/v2\nentities:\n  person: {}\n");
    let pkg_b = emit_from_source("format: typebridge.schema/v2\nentities:\n  company: {}\n");

    write_package(&pkg_a, &gen_a);
    write_package(&pkg_b, &gen_b);

    // Give generated_b a distinct package name in Cargo.toml so Cargo lockfile distinguishes package A and package B
    let cargo_b_path = gen_b.join("Cargo.toml");
    let cargo_b_content = fs::read_to_string(&cargo_b_path).unwrap();
    fs::write(
        &cargo_b_path,
        cargo_b_content.replace(
            "name = \"type-bridge-generated-schema\"",
            "name = \"type-bridge-generated-schema-b\"",
        ),
    )
    .unwrap();

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(cross_neg.join("src")).unwrap();
    fs::write(
        cross_neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"cross-schema-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nschema_a = {{ package = \"type-bridge-generated-schema\", path = \"../generated_a\" }}\nschema_b = {{ package = \"type-bridge-generated-schema-b\", path = \"../generated_b\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let cross_neg_source = r#"
use type_bridge::Database;

fn expects_schema_a_database(_: Database<schema_a::AppSchema>) {}

fn mix_database_handles(db_b: Database<schema_b::AppSchema>) {
    expects_schema_a_database(db_b);
}

fn expects_schema_a_function(_: schema_a::FunctionToken<schema_a::AppSchema, (), ()>) {}

fn mix_function_tokens(token_b: schema_b::FunctionToken<schema_b::AppSchema, (), ()>) {
    expects_schema_a_function(token_b);
}

fn expects_schema_a_model(_: schema_a::PersonRef) {}

fn mix_models(person_b: schema_b::PersonRef) {
    expects_schema_a_model(person_b);
}

fn main() {}
"#;
    fs::write(cross_neg.join("src/main.rs"), cross_neg_source).unwrap();

    let manifest = cross_neg.join("Cargo.toml");
    let output = cargo(
        &["check", "--manifest-path", manifest.to_str().unwrap()],
        &stage.path().join("cross-neg-target"),
    );

    assert!(
        !output.status.success(),
        "cross-schema consumer unexpectedly compiled when mixing schemas"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mismatched types"),
        "cross-schema failure was not a type mismatch:\n{stderr}"
    );
    assert!(
        stderr.contains("schema_a::AppSchema") || stderr.contains("schema_b::AppSchema"),
        "cross-schema failure did not reference distinct schema markers:\n{stderr}"
    );
}

#[test]
fn abstract_models_cannot_be_constructed_directly() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("abstract_negative");

    let pkg = emit_from_source(
        "format: typebridge.schema/v2\nentities:\n  party:\n    abstract: true\n  person:\n    sub: party\n",
    );
    write_package(&pkg, &generated_dir);

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(neg.join("src")).unwrap();
    fs::write(
        neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"abstract-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let neg_source = r#"
fn main() {
    let _ = generated::Party { _private: () };
}
"#;
    fs::write(neg.join("src/main.rs"), neg_source).unwrap();

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("abstract-neg-target"),
    );

    assert!(
        !output.status.success(),
        "abstract model unexpectedly constructed directly"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private") || stderr.contains("field"),
        "abstract construction failure was not a private field error:\n{stderr}"
    );
}

#[test]
fn family_child_only_fields_fail_at_compile_time() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("family_negative");

    let pkg = emit_from_source(
        "format: typebridge.schema/v2\nattributes:\n  age: { value: integer }\nentities:\n  party:\n    abstract: true\n  person:\n    sub: party\n    owns:\n      age: { card: 1 }\n  company:\n    sub: party\n",
    );
    write_package(&pkg, &generated_dir);

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(neg.join("src")).unwrap();
    fs::write(
        neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"family-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let neg_source = r#"
use generated::PartyFamily;

fn access_child_only_field(family: PartyFamily) {
    let _ = family.age();
}

fn main() {}
"#;
    fs::write(neg.join("src/main.rs"), neg_source).unwrap();

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("family-neg-target"),
    );

    assert!(
        !output.status.success(),
        "family unexpectedly exposed child-only field directly"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no method named `age`") || stderr.contains("PartyFamily"),
        "family child-only failure was not a missing method error:\n{stderr}"
    );
}

#[test]
fn complete_model_cannot_be_passed_to_relation_create() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("relation_create_negative");

    let pkg = emit_from_source(
        "format: typebridge.schema/v2\nentities:\n  person: {}\nrelations:\n  employment:\n    relates:\n      employee: { card: 1 }\nplays:\n  person:\n    employment:\n      employee: { card: 1 }\n",
    );
    write_package(&pkg, &generated_dir);

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(neg.join("src")).unwrap();
    fs::write(
        neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"relation-create-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let neg_source = r#"
use generated::{EmploymentCreate, Person};

fn pass_complete_model_to_relation_create(person: Person) {
    let _ = EmploymentCreate::new(person);
}

fn main() {}
"#;
    fs::write(neg.join("src/main.rs"), neg_source).unwrap();

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("relation-create-neg-target"),
    );

    assert!(
        !output.status.success(),
        "relation create unexpectedly accepted complete model instead of reference"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mismatched types") || stderr.contains("PersonRef"),
        "relation create failure was not a type mismatch diagnostic:\n{stderr}"
    );
}

#[test]
fn entity_cannot_satisfy_relation_model_bound() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("kind_negative");

    let pkg = emit_from_source(
        "format: typebridge.schema/v2\nentities:\n  person: {}\nrelations:\n  membership: {}\n",
    );
    write_package(&pkg, &generated_dir);

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(neg.join("src")).unwrap();
    fs::write(
        neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"kind-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let neg_source = r#"
use type_bridge::model::RelationModel;
use generated::Person;

fn requires_relation_model<T: RelationModel>() {}

fn main() {
    requires_relation_model::<Person>();
}
"#;
    fs::write(neg.join("src/main.rs"), neg_source).unwrap();

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("kind-neg-target"),
    );

    assert!(
        !output.status.success(),
        "entity model unexpectedly satisfied RelationModel bound"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("RelationModel") || stderr.contains("trait bound"),
        "kind boundary failure was not a RelationModel trait bound error:\n{stderr}"
    );
}

#[test]
fn complete_model_has_no_public_from_parts_constructor() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("from_parts_negative");

    let pkg = emit_from_source("format: typebridge.schema/v2\nentities:\n  person: {}\n");
    write_package(&pkg, &generated_dir);

    let rust_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_crate.to_string_lossy().replace('\\', "\\\\");

    fs::create_dir_all(neg.join("src")).unwrap();
    fs::write(
        neg.join("Cargo.toml"),
        format!(
            "[package]\nname = \"from-parts-negative\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust_path}\", default-features = false }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust_path}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();

    let neg_source = r#"
use generated::Person;

fn main() {
    let _ = Person::from_parts("iid-1".to_string());
}
"#;
    fs::write(neg.join("src/main.rs"), neg_source).unwrap();

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("from-parts-neg-target"),
    );

    assert!(
        !output.status.success(),
        "complete model unexpectedly exposed a public from_parts constructor"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("from_parts") || stderr.contains("no function or associated item"),
        "from_parts failure was not a missing function error:\n{stderr}"
    );
}

#[test]
fn entity_ref_cannot_satisfy_entity_model_bound() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("entity_ref_neg");

    let pkg = emit_from_source("format: typebridge.schema/v2\nentities:\n  person: {}\n");
    write_package(&pkg, &generated_dir);
    write_consumer(
        &neg,
        "entity-ref-neg",
        "use type_bridge::model::EntityModel;\nuse generated::PersonRef;\nfn check<T: EntityModel>() {}\nfn main() { check::<PersonRef>(); }\n",
    );

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("entity-ref-neg-target"),
    );

    assert!(
        !output.status.success(),
        "entity ref unexpectedly satisfied EntityModel bound"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EntityModel") || stderr.contains("trait bound"),
        "entity ref failure was not an EntityModel bound error:\n{stderr}"
    );
}

#[test]
fn relation_ref_cannot_satisfy_relation_model_bound() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("relation_ref_neg");

    let pkg = emit_from_source("format: typebridge.schema/v2\nrelations:\n  membership: {}\n");
    write_package(&pkg, &generated_dir);
    write_consumer(
        &neg,
        "relation-ref-neg",
        "use type_bridge::model::RelationModel;\nuse generated::MembershipRef;\nfn check<T: RelationModel>() {}\nfn main() { check::<MembershipRef>(); }\n",
    );

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("relation-ref-neg-target"),
    );

    assert!(
        !output.status.success(),
        "relation ref unexpectedly satisfied RelationModel bound"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("RelationModel") || stderr.contains("trait bound"),
        "relation ref failure was not a RelationModel bound error:\n{stderr}"
    );
}

#[test]
fn relation_cannot_satisfy_entity_model_bound() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("relation_entity_neg");

    let pkg = emit_from_source("format: typebridge.schema/v2\nrelations:\n  membership: {}\n");
    write_package(&pkg, &generated_dir);
    write_consumer(
        &neg,
        "relation-entity-neg",
        "use type_bridge::model::EntityModel;\nuse generated::Membership;\nfn check<T: EntityModel>() {}\nfn main() { check::<Membership>(); }\n",
    );

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("relation-entity-neg-target"),
    );

    assert!(
        !output.status.success(),
        "relation model unexpectedly satisfied EntityModel bound"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EntityModel") || stderr.contains("trait bound"),
        "relation entity failure was not an EntityModel bound error:\n{stderr}"
    );
}

#[test]
fn abstract_model_does_not_implement_complete_model() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let neg = stage.path().join("abstract_complete_neg");

    let pkg =
        emit_from_source("format: typebridge.schema/v2\nentities:\n  thing:\n    abstract: true\n");
    write_package(&pkg, &generated_dir);
    write_consumer(
        &neg,
        "abstract-complete-neg",
        "use type_bridge::model::CompleteModel;\nuse generated::Thing;\nfn check<T: CompleteModel>() {}\nfn main() { check::<Thing>(); }\n",
    );

    let output = cargo(
        &[
            "check",
            "--manifest-path",
            neg.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("abstract-complete-neg-target"),
    );

    assert!(
        !output.status.success(),
        "abstract model unexpectedly satisfied CompleteModel bound"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CompleteModel") || stderr.contains("trait bound"),
        "abstract complete failure was not a CompleteModel bound error:\n{stderr}"
    );
}

#[test]
fn rust_emitter_name_collision_detected_for_colliding_derived_names() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-collision.yaml").unwrap(),
        "format: typebridge.schema/v2\nentities:\n  person:\n    abstract: true\n  employee:\n    sub: person\n  person_family: {}\n",
    )]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
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
    let err = emitter.emit(&projection).unwrap_err();
    assert_eq!(err.code().as_str(), "rust_emitter_name_collision");
}

#[test]
fn rust_acceptance_review_06b_capability_boundary_is_real() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let pkg = emit_from_source("format: typebridge.schema/v2\nentities:\n  person: {}\n");
    write_package(&pkg, &generated_dir);

    let failures = [
        (
            "cap-private-new",
            &[][..],
            "use type_bridge::__codegen::HydrationCapability;\nfn main() { let _ = HydrationCapability::new(); }\n",
            ["HydrationCapability", "new", "private"],
        ),
        (
            "cap-private-field",
            &[][..],
            "use type_bridge::__codegen::HydrationCapability;\nfn main() { let _ = HydrationCapability { _private: () }; }\n",
            ["HydrationCapability", "_private", "private"],
        ),
        (
            "cap-helper-default-off",
            &[][..],
            "use type_bridge::__codegen::materialize_model_for_test;\nfn main() { let _ = materialize_model_for_test::<generated::Person>; }\n",
            [
                "materialize_model_for_test",
                "unresolved import",
                "test-harness",
            ],
        ),
        (
            "cap-private-new-with-harness",
            &["test-harness"][..],
            "use type_bridge::__codegen::HydrationCapability;\nfn main() { let _ = HydrationCapability::new(); }\n",
            ["HydrationCapability", "new", "private"],
        ),
    ];

    for (name, features, source, expected) in failures {
        let consumer = stage.path().join(name);
        write_consumer_with_features(&consumer, name, source, features);
        let output = cargo(
            &[
                "check",
                "--manifest-path",
                consumer.join("Cargo.toml").to_str().unwrap(),
            ],
            &stage.path().join(format!("{name}-target")),
        );
        assert!(
            !output.status.success(),
            "{name} unexpectedly crossed the capability boundary"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        for needle in expected {
            assert!(
                stderr.contains(needle),
                "{name} did not report `{needle}`:\n{stderr}"
            );
        }
        println!("06B CAPABILITY {name}: {}", stderr.trim());
    }
}

#[test]
fn rust_acceptance_review_06a_semantics_and_owns_constraints() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let consumer = stage.path().join("consumer_06a");

    let schema_yaml = r#"format: typebridge.schema/v2
attributes:
  attribute_pattern:
    value:
      type: string
      regex: "^attr-[a-z]+$"
  decimal_window:
    value:
      type: decimal
      range:
        min: "2"
        max: "10"
  attribute_choice:
    value:
      type: integer
      values: [2, 4]
  zero_choice:
    value:
      type: double
      values: [0.0]
  duration_choice:
    value:
      type: duration
      values: ["P1D", "P2D"]
  inherited_score: { value: integer }
  shared_score: { value: integer }
  edge_text: { value: string }
  edge_choice: { value: integer }
  timezone_value: { value: datetime-tz }
  plain_text: { value: string }
  scalar_double: { value: double }
  scalar_decimal: { value: decimal }
  scalar_date: { value: date }
  scalar_datetime: { value: datetime }
  scalar_datetime_tz: { value: datetime-tz }
  scalar_duration: { value: duration }
entities:
  base_owner:
    abstract: true
    owns:
      inherited_score:
        card: 1
        range:
          min: 10
          max: 50
  child_owner:
    sub: base_owner
  alpha_owner:
    owns:
      shared_score:
        card: 1
        range:
          min: 20
          max: 40
  beta_owner:
    owns:
      shared_score:
        card: 1
        range:
          min: 60
          max: 90
  edge_regex_owner:
    owns:
      edge_text:
        card: { min: 1, max: 3 }
        regex: "^edge-[a-z]+$"
  edge_values_owner:
    owns:
      edge_choice:
        card: 1
        values: [7, 9]
  timezone_owner:
    owns:
      timezone_value:
        card: { min: 0, max: 1 }
        range:
          min: "2026-07-28T04:00:00Z"
          max: "2026-07-28T06:00:00Z"
  attribute_regex_owner:
    owns:
      attribute_pattern: { card: 1 }
  attribute_range_owner:
    owns:
      decimal_window: { card: 1 }
  attribute_values_owner:
    owns:
      attribute_choice: { card: 1 }
  text_owner:
    owns:
      plain_text: { card: 1 }
"#;

    let projection = project_from_source(schema_yaml);
    let child = type_bridge_contract::id::TypeId::new(
        type_bridge_contract::id::TypeKind::Entity,
        "child_owner",
    )
    .unwrap();
    let base = type_bridge_contract::id::TypeId::new(
        type_bridge_contract::id::TypeKind::Entity,
        "base_owner",
    )
    .unwrap();
    let attribute = type_bridge_contract::id::AttributeId::new("inherited_score").unwrap();
    let effective =
        type_bridge_contract::schema::OwnsFactId::new(child.clone(), attribute.clone()).unwrap();
    let declaring = type_bridge_contract::schema::OwnsFactId::new(base, attribute).unwrap();
    let token = &projection.models()[&child].query_tokens().fields()[&effective];
    assert_eq!(token.id(), &effective);
    assert_eq!(token.declaring_id(), &declaring);

    let pkg = RustEmitter::new().emit(&projection).unwrap();
    let tokens_rs = std::str::from_utf8(pkg.files().get("src/tokens.rs").unwrap()).unwrap();
    let inherited_token_line = tokens_rs
        .lines()
        .find(|line| {
            line.contains("pub const inherited_score")
                && line.contains("\\\"label\\\":\\\"child_owner\\\"")
        })
        .unwrap();
    assert!(inherited_token_line.contains("\\\"label\\\":\\\"child_owner\\\""));
    assert!(inherited_token_line.contains("\\\"label\\\":\\\"base_owner\\\""));
    let create_rs = std::str::from_utf8(pkg.files().get("src/create.rs").unwrap()).unwrap();
    let inherited_encoded_line = create_rs
        .lines()
        .find(|line| {
            line.contains("fields.push")
                && line.contains("inherited_score")
                && line.contains("base_owner")
        })
        .unwrap();
    assert!(inherited_encoded_line.contains("\\\"label\\\":\\\"base_owner\\\""));
    let read_rs = std::str::from_utf8(pkg.files().get("src/read.rs").unwrap()).unwrap();
    let attribute_literal_line = read_rs
        .lines()
        .find(|line| {
            line.contains(
                r#"Decimal::try_new("2").map_err(|__tb_error| prefix_validation_path(__tb_error, &ValidationPath::root().join("value")))?"#,
            )
        })
        .expect("attribute-value checked scalar literal uses its active value path");
    let owner_create_literal_line = create_rs
        .lines()
        .find(|line| {
            line.contains(
                r#"DateTimeTz::try_new("2026-07-28T04:00:00Z").map_err(|__tb_error| prefix_validation_path(__tb_error, &__tb_path))?"#,
            )
        })
        .expect("owner-create checked scalar literal uses its active field path");
    let materializer_literal_line = read_rs
        .lines()
        .find(|line| {
            line.contains(
                r#"DateTimeTz::try_new("2026-07-28T04:00:00Z").map_err(|__tb_error| prefix_validation_path(__tb_error, &__tb_member_path))?"#,
            )
        })
        .expect("materializer checked scalar literal uses its active field path");
    println!("FRESH inherited token: {}", inherited_token_line.trim());
    println!(
        "FRESH inherited encoded field: {}",
        inherited_encoded_line.trim()
    );
    println!(
        "FRESH attribute-value literal: {}",
        attribute_literal_line.trim()
    );
    println!(
        "FRESH owner-create literal: {}",
        owner_create_literal_line.trim()
    );
    println!(
        "FRESH materializer literal: {}",
        materializer_literal_line.trim()
    );
    write_package(&pkg, &generated_dir);

    let main_rs = r#"use generated::*;
use type_bridge::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, EncodedScalar,
    HydratedRow, IntoEncodedCreate, IntoEncodedScalar, ValidationError,
    materialize_model_for_test,
};

fn scalar_double(value: f64) -> Result<ScalarDouble, ValidationError> {
    ScalarDouble::new(CanonicalDouble::try_new(value)?)
}

fn scalar_decimal(value: &str) -> Result<ScalarDecimal, ValidationError> {
    ScalarDecimal::new(Decimal::try_new(value)?)
}

fn scalar_date(value: &str) -> Result<ScalarDate, ValidationError> {
    ScalarDate::new(Date::try_new(value)?)
}

fn scalar_datetime(value: &str) -> Result<ScalarDatetime, ValidationError> {
    ScalarDatetime::new(DateTime::try_new(value)?)
}

fn scalar_datetime_tz(value: &str) -> Result<ScalarDatetimeTz, ValidationError> {
    ScalarDatetimeTz::new(DateTimeTz::try_new(value)?)
}

fn scalar_duration(value: &str) -> Result<ScalarDuration, ValidationError> {
    ScalarDuration::new(Duration::try_new(value)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Attribute-value @regex, @range, and @values.
    assert_eq!(AttributePattern::new("attr-good")?.value(), "attr-good");
    let err = AttributePattern::new("wrong").unwrap_err();
    assert_eq!((err.code(), err.field()), ("regex_violation", "value"));

    assert_eq!(DecimalWindow::new(Decimal::try_new("3")?)?.value().as_str(), "3");
    let err = DecimalWindow::new(Decimal::try_new("11")?).unwrap_err();
    assert_eq!((err.code(), err.field()), ("range_violation", "value"));

    assert_eq!(*AttributeChoice::new(4i64)?.value(), 4);
    let err = AttributeChoice::new(3i64).unwrap_err();
    assert_eq!((err.code(), err.field()), ("values_violation", "value"));

    // Exact-bit signed zero identity is distinct, while generated @values uses semantic equality.
    let negative_zero = CanonicalDouble::try_new(-0.0)?;
    let positive_zero = CanonicalDouble::try_new(0.0)?;
    assert_ne!(negative_zero.to_bits(), positive_zero.to_bits());
    assert_eq!(ZeroChoice::new(negative_zero)?.value().to_bits(), (-0.0f64).to_bits());
    assert_eq!(
        DurationChoice::new(Duration::try_new("P1D")?)?
            .value()
            .as_str(),
        "P1D"
    );
    let err = DurationChoice::new(Duration::try_new("P3D")?).unwrap_err();
    assert_eq!((err.code(), err.field()), ("values_violation", "value"));

    // Owns-edge @range differs for the same attribute under two owners.
    let score_30 = SharedScore::new(30i64)?;
    assert_eq!(*AlphaOwnerCreate::new(score_30.clone())?.shared_score().value(), 30);
    let err = BetaOwnerCreate::new(score_30.clone()).unwrap_err();
    assert_eq!((err.code(), err.field()), ("range_violation", "shared_score"));
    assert!(BetaOwnerCreate::new(SharedScore::new(75i64)?).is_ok());
    assert!(AlphaOwnerCreate::new(SharedScore::new(75i64)?).is_err());

    // True inherited owns edge: child_owner does not redeclare inherited_score.
    assert!(ChildOwnerType::inherited_score.owns_id_json().contains("base_owner"));
    assert!(ChildOwnerType::inherited_score.metadata_json().contains("child_owner"));
    let inherited = InheritedScore::new(25i64)?;
    let encoded = ChildOwnerCreate::new(inherited.clone())?.into_encoded_create()?;
    assert!(encoded.fields()[0].0.contains("base_owner"));
    let err = ChildOwnerCreate::new(InheritedScore::new(7i64)?).unwrap_err();
    assert_eq!((err.code(), err.field()), ("range_violation", "inherited_score"));

    // Owns-edge @regex preserves a dynamic sequence index.
    let edge_ok = EdgeText::new("edge-good")?;
    let edge_bad = EdgeText::new("bad")?;
    assert!(EdgeRegexOwnerCreate::new(vec![edge_ok.clone()]).is_ok());
    let err = EdgeRegexOwnerCreate::new(vec![edge_ok.clone(), edge_bad.clone()]).unwrap_err();
    assert_eq!((err.code(), err.field()), ("regex_violation", "edge_text[1]"));

    // Owns-edge @values.
    assert!(EdgeValuesOwnerCreate::new(EdgeChoice::new(7i64)?).is_ok());
    let err = EdgeValuesOwnerCreate::new(EdgeChoice::new(8i64)?).unwrap_err();
    assert_eq!((err.code(), err.field()), ("values_violation", "edge_choice"));

    // UTC semantic ordering: spelling sorts below 04:00Z, instant is 05:30Z and in range.
    let timezone = TimezoneValue::new(DateTimeTz::try_new(
        "2026-07-28T01:30:00-04:00",
    )?)?;
    assert!(TimezoneOwnerCreate::new(Some(timezone.clone())).is_ok());
    assert!(TimezoneOwnerCreate::new(None).is_ok());

    // Checked scalar wrappers make invalid representations fail on generated attribute paths.
    assert_eq!(scalar_double(f64::NAN).unwrap_err().code(), "noncanonical_double");
    assert_eq!(scalar_double(f64::INFINITY).unwrap_err().code(), "noncanonical_double");
    assert_eq!(scalar_double(f64::NEG_INFINITY).unwrap_err().code(), "noncanonical_double");
    assert_eq!(scalar_decimal("1.00").unwrap_err().code(), "noncanonical_decimal");
    assert_eq!(scalar_date("2026-99-99").unwrap_err().code(), "noncanonical_date");
    assert_eq!(scalar_datetime("not-a-datetime").unwrap_err().code(), "noncanonical_datetime");
    assert_eq!(scalar_datetime_tz("bad-tz").unwrap_err().code(), "noncanonical_datetime_tz");
    assert_eq!(scalar_duration("bad-duration").unwrap_err().code(), "noncanonical_duration");
    assert!(scalar_double(1.5).is_ok());
    assert!(scalar_decimal("1.5").is_ok());
    assert!(scalar_date("2026-07-28").is_ok());
    assert!(scalar_datetime("2026-07-28T03:55:00").is_ok());
    assert!(scalar_datetime_tz("2026-07-28T03:55:00Z").is_ok());
    assert!(scalar_duration("P1D").is_ok());

    // Attribute wrapper failures from hydration are nested under the owner field.
    let attribute_regex_row = HydratedRow::new(
        AttributeRegexOwner::TYPE_ID_JSON,
        "attribute-regex-iid".to_owned(),
        vec![(
            AttributeRegexOwnerType::attribute_pattern.owns_id_json(),
            vec![EncodedScalar::String("wrong".to_owned())],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<AttributeRegexOwner>(&attribute_regex_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("regex_violation", "attribute_pattern"));

    let attribute_range_row = HydratedRow::new(
        AttributeRangeOwner::TYPE_ID_JSON,
        "attribute-range-iid".to_owned(),
        vec![(
            AttributeRangeOwnerType::decimal_window.owns_id_json(),
            vec![Decimal::try_new("11")?.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<AttributeRangeOwner>(&attribute_range_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("range_violation", "decimal_window"));

    let attribute_values_row = HydratedRow::new(
        AttributeValuesOwner::TYPE_ID_JSON,
        "attribute-values-iid".to_owned(),
        vec![(
            AttributeValuesOwnerType::attribute_choice.owns_id_json(),
            vec![3i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<AttributeValuesOwner>(&attribute_values_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("values_violation", "attribute_choice"));

    // Owns-edge materialization checks use the same descriptors and current field/index paths.
    let edge_regex_row = HydratedRow::new(
        EdgeRegexOwner::TYPE_ID_JSON,
        "edge-regex-iid".to_owned(),
        vec![(
            EdgeRegexOwnerType::edge_text.owns_id_json(),
            vec![
                "edge-good".to_owned().into_encoded_scalar(),
                "bad".to_owned().into_encoded_scalar(),
            ],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<EdgeRegexOwner>(&edge_regex_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("regex_violation", "edge_text[1]"));
    let edge_regex_domain_row = HydratedRow::new(
        EdgeRegexOwner::TYPE_ID_JSON,
        "edge-regex-domain-iid".to_owned(),
        vec![(
            EdgeRegexOwnerType::edge_text.owns_id_json(),
            vec![1i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<EdgeRegexOwner>(&edge_regex_domain_row).unwrap_err();
    assert_eq!(
        (err.code(), err.field()),
        ("wrong_scalar_domain", "edge_text[0]")
    );

    let alpha_row = HydratedRow::new(
        AlphaOwner::TYPE_ID_JSON,
        "alpha-iid".to_owned(),
        vec![(
            AlphaOwnerType::shared_score.owns_id_json(),
            vec![75i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<AlphaOwner>(&alpha_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("range_violation", "shared_score"));

    let edge_values_row = HydratedRow::new(
        EdgeValuesOwner::TYPE_ID_JSON,
        "edge-values-iid".to_owned(),
        vec![(
            EdgeValuesOwnerType::edge_choice.owns_id_json(),
            vec![8i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<EdgeValuesOwner>(&edge_values_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("values_violation", "edge_choice"));

    let child_row = HydratedRow::new(
        ChildOwner::TYPE_ID_JSON,
        "child-iid".to_owned(),
        vec![(
            ChildOwnerType::inherited_score.owns_id_json(),
            vec![25i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let child: ChildOwner = materialize_model_for_test(&child_row)?;
    assert_eq!(*child.inherited_score().value(), 25);
    let inherited_failure_row = HydratedRow::new(
        ChildOwner::TYPE_ID_JSON,
        "child-invalid-iid".to_owned(),
        vec![(
            ChildOwnerType::inherited_score.owns_id_json(),
            vec![7i64.into_encoded_scalar()],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<ChildOwner>(&inherited_failure_row).unwrap_err();
    assert_eq!(
        (err.code(), err.field()),
        ("range_violation", "inherited_score")
    );

    let timezone_row = HydratedRow::new(
        TimezoneOwner::TYPE_ID_JSON,
        "timezone-iid".to_owned(),
        vec![(
            TimezoneOwnerType::timezone_value.owns_id_json(),
            vec![timezone.value().into_encoded_scalar()],
        )],
        vec![],
    );
    let timezone_read: TimezoneOwner = materialize_model_for_test(&timezone_row)?;
    assert_eq!(
        timezone_read.timezone_value().unwrap().value().as_str(),
        "2026-07-28T01:30:00-04:00"
    );

    // An unannotated generated string attribute still enforces canonical byte size.
    let oversized = "x".repeat(1024 * 1024 + 1);
    let err = PlainText::new(oversized.clone()).unwrap_err();
    assert_eq!((err.code(), err.field()), ("string_limit_exceeded", "value"));
    let oversized_row = HydratedRow::new(
        TextOwner::TYPE_ID_JSON,
        "text-iid".to_owned(),
        vec![(
            TextOwnerType::plain_text.owns_id_json(),
            vec![EncodedScalar::String(oversized)],
        )],
        vec![],
    );
    let err = materialize_model_for_test::<TextOwner>(&oversized_row).unwrap_err();
    assert_eq!((err.code(), err.field()), ("string_limit_exceeded", "plain_text"));

    println!("Review 06A semantics and owns constraint probes PASSED.");
    Ok(())
}
"#;

    write_consumer_with_features(&consumer, "consumer-06a", main_rs, &["test-harness"]);

    let output = cargo(
        &[
            "run",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("consumer-06a-target"),
    );

    assert!(
        output.status.success(),
        "Review 06A consumer execution failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rust_acceptance_review_06b_namespace_collisions_are_diagnostic() {
    let fixtures = [
        (
            "family downcast versus member",
            r#"format: typebridge.schema/v2
attributes:
  as-alpha: { value: string }
entities:
  family-root:
    abstract: true
    owns:
      as-alpha: { card: 1 }
  alpha: { sub: family-root }
  beta: { sub: family-root }
"#,
            ["common family member `as_alpha`", "family downcast to"],
        ),
        (
            "complete reference versus member",
            r#"format: typebridge.schema/v2
attributes:
  reference: { value: string }
entities:
  person:
    owns:
      reference: { card: 1 }
"#,
            ["complete reference getter", "complete field"],
        ),
        (
            "reference constructor versus key getter",
            r#"format: typebridge.schema/v2
attributes:
  from-key: { value: string }
entities:
  person:
    owns:
      from-key: { key: true }
"#,
            ["reference key getter", "single-key constructor"],
        ),
        (
            "relation create union versus public type",
            r#"format: typebridge.schema/v2
entities:
  mixed-link-participant-ref: {}
  person: {}
  robot: {}
relations:
  mixed-link:
    relates:
      participant: { card: 1 }
plays:
  person:
    mixed-link:
      participant: { card: 1 }
  robot:
    mixed-link:
      participant: { card: 1 }
"#,
            ["create reference union", "complete model"],
        ),
        (
            "materializer versus member",
            r#"format: typebridge.schema/v2
attributes:
  materialize: { value: string }
entities:
  person:
    owns:
      materialize: { card: 1 }
"#,
            ["complete materializer", "complete field"],
        ),
        (
            "create codec versus member",
            r#"format: typebridge.schema/v2
attributes:
  into-encoded-create: { value: string }
entities:
  person:
    owns:
      into-encoded-create: { card: 1 }
"#,
            ["create codec", "create member `into_encoded_create`"],
        ),
        (
            "runtime DTO versus model",
            "format: typebridge.schema/v2\nentities:\n  hydrated-player: {}\n",
            [
                "generated Rust runtime export `HydratedPlayer`",
                "complete model",
            ],
        ),
        (
            "runtime trait versus model",
            "format: typebridge.schema/v2\nentities:\n  into-encoded-scalar: {}\n",
            [
                "generated Rust runtime export `IntoEncodedScalar`",
                "complete model",
            ],
        ),
        (
            "prelude type versus model",
            "format: typebridge.schema/v2\nentities:\n  option: {}\n",
            ["generated Rust runtime export `Option`", "complete model"],
        ),
        (
            "feature-gated helper versus function",
            r#"format: typebridge.schema/v2
functions:
  materialize-model-for-test:
    returns: { stream: [integer] }
    body: { typeql: "match let $x = 42; return first $x;" }
"#,
            [
                "generated Rust runtime export `materialize_model_for_test`",
                "function",
            ],
        ),
        (
            "implementation-local prefix versus key",
            r#"format: typebridge.schema/v2
attributes:
  __tb-path: { value: string }
entities:
  person:
    owns:
      __tb-path: { key: true }
"#,
            [
                "generated implementation binding prefix __tb_",
                "query member",
            ],
        ),
    ];

    for (label, source, identities) in fixtures {
        let projection = project_from_source(source);
        let error = RustEmitter::new().emit(&projection).unwrap_err();
        assert_eq!(error.code().as_str(), "rust_emitter_name_collision");
        for identity in identities {
            assert!(
                error.message().contains(identity),
                "{label} omitted identity `{identity}`: {}",
                error.message()
            );
        }
        println!("06B COLLISION {label}: {}", error.message());
    }
}

#[test]
fn rust_acceptance_review_06b_dispatch_helper_collision_is_diagnostic() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("helper.yaml").unwrap(),
        "format: typebridge.schema/v2\nattributes:\n  __tb-dispatch-subtype: { value: string }\nentities:\n  person:\n    owns:\n      __tb-dispatch-subtype: { card: 1 }\n",
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
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
    let error = emitter.emit(&projection).unwrap_err();
    assert_eq!(error.code().as_str(), "rust_emitter_name_collision");
    assert!(
        error
            .to_string()
            .contains("generated implementation binding prefix __tb_")
    );
}

#[test]
fn rust_acceptance_review_06b_internal_bindings_are_hygienic() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let consumer = stage.path().join("consumer_06b_hygiene");
    let schema_yaml = r#"format: typebridge.schema/v2
attributes:
  path: { value: string }
  player: { value: string }
  seen-keys: { value: string }
entities:
  person:
    owns:
      path:
        card: 1
        regex: '^x+$'
  keyed:
    owns:
      player: { key: true }
      seen-keys: { key: true }
relations:
  holder:
    relates:
      participant: { card: 1 }
plays:
  keyed:
    holder:
      participant: { card: 1 }
"#;

    let pkg = emit_from_source(schema_yaml);
    let create_rs = std::str::from_utf8(pkg.files().get("src/create.rs").unwrap()).unwrap();
    let read_rs = std::str::from_utf8(pkg.files().get("src/read.rs").unwrap()).unwrap();
    let reference_rs = std::str::from_utf8(pkg.files().get("src/reference.rs").unwrap()).unwrap();
    assert!(create_rs.contains("let __tb_member = &path;"));
    assert!(create_rs.contains("let __tb_path = ValidationPath::root().join(\"path\");"));
    assert!(read_rs.contains("fn materialize(__tb_row: &HydratedRow"));
    assert!(reference_rs.contains("fn __tb_from_player(__tb_player: &HydratedPlayer"));
    assert!(reference_rs.contains("let mut __tb_seen_keys"));
    assert!(reference_rs.contains("let player = if let Some"));
    assert!(reference_rs.contains("let seen_keys = if let Some"));

    for line in [
        create_rs
            .lines()
            .find(|line| line.contains("let __tb_member = &path"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("fn __tb_from_player(__tb_player"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("let player = if let Some"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("let seen_keys = if let Some"))
            .unwrap(),
    ] {
        println!("FRESH 06B HYGIENE: {}", line.trim());
    }

    write_package(&pkg, &generated_dir);
    let main_rs = r#"use generated::*;
use type_bridge::__codegen::{
    HydratedPlayer, HydratedRow, IntoEncodedCreate, IntoEncodedScalar,
    materialize_model_for_test,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new("xxx")?;
    let encoded = PersonCreate::new(path)?.into_encoded_create()?;
    assert_eq!(encoded.fields().len(), 1);
    let error = PersonCreate::new(Path::new("not-x")?).unwrap_err();
    assert_eq!((error.code(), error.field()), ("regex_violation", "path"));

    let player = Player::new("player-key")?;
    let seen_keys = SeenKeys::new("seen-key")?;
    let hydrated = HydratedPlayer::new(
        Keyed::TYPE_ID_JSON,
        Some("keyed-iid".to_owned()),
        vec![
            (
                KeyedType::player.owns_id_json(),
                player.value().into_encoded_scalar(),
            ),
            (
                KeyedType::seen_keys.owns_id_json(),
                seen_keys.value().into_encoded_scalar(),
            ),
        ],
    );
    let row = HydratedRow::new(
        Holder::TYPE_ID_JSON,
        "holder-iid".to_owned(),
        vec![],
        vec![(HolderType::participant.role_id_json(), vec![hydrated])],
    );
    let holder: Holder = materialize_model_for_test(&row)?;
    let HolderParticipantPlayer::Keyed(keyed) = holder.participant();
    assert!(keyed.player().is_some());
    assert!(keyed.seen_keys().is_some());

    println!("Review 06B continuation 01 hygiene package PASSED.");
    Ok(())
}
"#;
    write_consumer_with_features(
        &consumer,
        "consumer-06b-hygiene",
        main_rs,
        &["test-harness"],
    );
    let output = cargo(
        &[
            "run",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("consumer-06b-hygiene-target"),
    );
    assert!(
        output.status.success(),
        "Review 06B hygiene consumer failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

#[test]
fn rust_acceptance_review_06b_reference_and_family_closure() {
    let stage = Stage::new();
    let generated_dir = stage.path().join("generated");
    let consumer = stage.path().join("consumer_06b");
    let schema_yaml = r#"format: typebridge.schema/v2
attributes:
  email: { value: string }
  external-id: { value: integer }
  inherited-key: { value: string }
  common-value: { value: string }
  child-only: { value: string }
  redeclared-value: { value: string }
  varying-values: { value: string }
entities:
  no-key: {}
  one-key:
    owns:
      email: { key: true }
  multi-key:
    owns:
      email: { key: true }
      external-id: { key: true }
  keyed-root:
    abstract: true
    owns:
      inherited-key: { key: true }
  keyed-child: { sub: keyed-root }
  family-root:
    abstract: true
    owns:
      common-value: { card: 1 }
      redeclared-value: { card: 1 }
      varying-values: { card: { min: 0, max: 3 } }
  alpha:
    sub: family-root
    owns:
      child-only: { card: { min: 0, max: 1 } }
  beta:
    sub: family-root
    owns:
      redeclared-value: { card: 1 }
      varying-values: { card: { min: 1, max: 2 } }
relations:
  single-link:
    relates:
      subject: { card: 1 }
  mixed-link:
    relates:
      participant: { card: 1 }
  reference-holder:
    relates:
      participant: { card: { min: 0, max: 8 } }
plays:
  one-key:
    single-link:
      subject: { card: 1 }
    mixed-link:
      participant: { card: 1 }
    reference-holder:
      participant: { card: 1 }
  no-key:
    mixed-link:
      participant: { card: 1 }
    reference-holder:
      participant: { card: 1 }
  multi-key:
    reference-holder:
      participant: { card: 1 }
  keyed-child:
    reference-holder:
      participant: { card: 1 }
"#;

    let projection = project_from_source(schema_yaml);
    let keyed_child = type_bridge_contract::id::TypeId::new(
        type_bridge_contract::id::TypeKind::Entity,
        "keyed-child",
    )
    .unwrap();
    let inherited_key = type_bridge_contract::id::AttributeId::new("inherited-key").unwrap();
    let effective =
        type_bridge_contract::schema::OwnsFactId::new(keyed_child.clone(), inherited_key.clone())
            .unwrap();
    let declaring = type_bridge_contract::schema::OwnsFactId::new(
        type_bridge_contract::id::TypeId::new(
            type_bridge_contract::id::TypeKind::Entity,
            "keyed-root",
        )
        .unwrap(),
        inherited_key,
    )
    .unwrap();
    let token = &projection.models()[&keyed_child].query_tokens().fields()[&effective];
    assert_eq!(token.declaring_id(), &declaring);

    let pkg = RustEmitter::new().emit(&projection).unwrap();
    let reference_rs = std::str::from_utf8(pkg.files().get("src/reference.rs").unwrap()).unwrap();
    let read_rs = std::str::from_utf8(pkg.files().get("src/read.rs").unwrap()).unwrap();
    let create_rs = std::str::from_utf8(pkg.files().get("src/create.rs").unwrap()).unwrap();

    assert!(!reference_rs.contains("pub fn try_new"));
    assert!(!reference_rs.contains("Option<Required"));
    assert!(!reference_rs.contains("EncodedReference::new"));
    assert!(!create_rs.contains("Either<"));
    assert!(!create_rs.contains("Either::"));
    assert!(reference_rs.contains("pub struct NoKeyRef"));
    assert!(reference_rs.contains("pub fn from_iid"));
    assert!(!reference_rs.contains("impl NoKeyRef {\n  pub fn from_key"));
    assert!(reference_rs.contains("email: Option<Email>"));
    assert!(reference_rs.contains("pub fn from_key(email: Email)"));
    assert!(reference_rs.contains("pub fn from_email(email: Email)"));
    assert!(reference_rs.contains("pub fn from_external_id(external_id: ExternalId)"));
    assert!(reference_rs.contains(&format!(
        "{:?}",
        String::from_utf8(type_bridge_contract::codec::to_canonical_json(&declaring).unwrap())
            .unwrap()
    )));

    let family_start = read_rs.find("pub enum FamilyRootFamily").unwrap();
    let family_end = read_rs[family_start..]
        .find("impl MaterializeModel for FamilyRootFamily")
        .unwrap()
        + family_start;
    let family_source = &read_rs[family_start..family_end];
    assert!(family_source.contains("pub fn common_value"));
    assert!(!family_source.contains("pub fn child_only"));
    assert!(!family_source.contains("pub fn redeclared_value"));
    assert!(!family_source.contains("pub fn varying_values"));
    assert!(family_source.contains("Alpha(Alpha)"));
    assert!(family_source.contains("Beta(Beta)"));
    assert!(!read_rs.contains("pub enum NoKeyFamily"));

    let fresh_lines = [
        reference_rs
            .lines()
            .find(|line| line.contains("pub struct NoKeyRef"))
            .unwrap(),
        reference_rs
            .lines()
            .skip_while(|line| !line.contains("pub struct NoKeyRef"))
            .find(|line| line.contains("pub fn from_iid"))
            .unwrap(),
        reference_rs
            .lines()
            .skip_while(|line| !line.contains("pub struct OneKeyRef"))
            .find(|line| line.contains("email: Option<Email>"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("pub fn from_key(email: Email)"))
            .unwrap(),
        reference_rs
            .lines()
            .skip_while(|line| !line.contains("pub struct MultiKeyRef"))
            .find(|line| line.contains("external_id: Option<ExternalId>"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("pub fn from_email(email: Email)"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("pub fn from_external_id(external_id: ExternalId)"))
            .unwrap(),
        read_rs
            .lines()
            .find(|line| line.contains("pub enum FamilyRootFamily"))
            .unwrap(),
        read_rs
            .lines()
            .find(|line| line.contains("pub fn common_value(&self)"))
            .unwrap(),
        create_rs
            .lines()
            .find(|line| line.contains("subject: Required<OneKeyRef>"))
            .unwrap(),
        create_rs
            .lines()
            .find(|line| line.contains("pub enum MixedLinkParticipantRef"))
            .unwrap(),
        create_rs
            .lines()
            .find(|line| line.contains("NoKey(NoKeyRef)"))
            .unwrap(),
        create_rs
            .lines()
            .find(|line| line.contains("OneKey(OneKeyRef)"))
            .unwrap(),
        reference_rs
            .lines()
            .find(|line| line.contains("keys.push") && line.contains("keyed-root"))
            .unwrap(),
    ];
    for line in fresh_lines {
        println!("FRESH 06B: {}", line.trim());
    }

    write_package(&pkg, &generated_dir);
    let main_rs = r#"use generated::*;
use type_bridge::__codegen::{
    EncodedScalar, HydratedPlayer, HydratedRow, IntoEncodedReference, IntoEncodedScalar,
    ValidationError, materialize_model_for_test,
};

fn player_error(player: HydratedPlayer) -> ValidationError {
    let row = HydratedRow::new(
        ReferenceHolder::TYPE_ID_JSON,
        "holder-iid".to_owned(),
        vec![],
        vec![(
            ReferenceHolderType::participant.role_id_json(),
            vec![player],
        )],
    );
    materialize_model_for_test::<ReferenceHolder>(&row).unwrap_err()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let no_key = NoKeyRef::from_iid("no-key-iid")?;
    let encoded = no_key.into_encoded_reference()?;
    assert_eq!(encoded.iid(), Some("no-key-iid"));
    assert!(encoded.keys().is_empty());

    let email = Email::new("one@example.test")?;
    let one_key = OneKeyRef::from_key(email.clone())?;
    let encoded = one_key.into_encoded_reference()?;
    assert_eq!(encoded.iid(), None);
    assert_eq!(encoded.keys().len(), 1);
    assert_eq!(encoded.keys()[0].0, OneKeyType::email.owns_id_json());

    let multi_email = MultiKeyRef::from_email(email.clone())?;
    assert!(multi_email.email().is_some());
    assert!(multi_email.external_id().is_none());
    let encoded = multi_email.into_encoded_reference()?;
    assert_eq!(encoded.keys().len(), 1);
    assert_eq!(encoded.keys()[0].0, MultiKeyType::email.owns_id_json());

    let external_id = ExternalId::new(42i64)?;
    let multi_external = MultiKeyRef::from_external_id(external_id.clone())?;
    assert!(multi_external.email().is_none());
    assert!(multi_external.external_id().is_some());
    let encoded = multi_external.into_encoded_reference()?;
    assert_eq!(encoded.keys().len(), 1);
    assert_eq!(encoded.keys()[0].0, MultiKeyType::external_id.owns_id_json());

    let inherited = InheritedKey::new("inherited")?;
    let inherited_ref = KeyedChildRef::from_key(inherited.clone())?;
    let encoded = inherited_ref.into_encoded_reference()?;
    assert!(encoded.keys()[0].0.contains("keyed-root"));
    assert_eq!(encoded.keys()[0].0, KeyedChildType::inherited_key.owns_id_json());

    let complete_row = HydratedRow::new(
        MultiKey::TYPE_ID_JSON,
        "multi-iid".to_owned(),
        vec![
            (
                MultiKeyType::email.owns_id_json(),
                vec![email.value().into_encoded_scalar()],
            ),
            (
                MultiKeyType::external_id.owns_id_json(),
                vec![external_id.value().into_encoded_scalar()],
            ),
        ],
        vec![],
    );
    let complete: MultiKey = materialize_model_for_test(&complete_row)?;
    let complete_ref = complete.reference();
    assert_eq!(complete_ref.iid(), Some("multi-iid"));
    assert!(complete_ref.email().is_some());
    assert!(complete_ref.external_id().is_some());
    let encoded = complete_ref.into_encoded_reference()?;
    assert_eq!(encoded.keys().len(), 2);

    let email_token = OneKeyType::email.owns_id_json();
    let external_token = MultiKeyType::external_id.owns_id_json();

    let err = player_error(HydratedPlayer::new(OneKey::TYPE_ID_JSON, None, vec![]));
    assert_eq!((err.code(), err.field()), ("missing_reference_identity", "participant[0]"));

    let err = player_error(HydratedPlayer::new(
        NoKey::TYPE_ID_JSON,
        Some("   ".to_owned()),
        vec![],
    ));
    assert_eq!((err.code(), err.field()), ("empty_iid", "participant[0].iid"));

    let err = player_error(HydratedPlayer::new(
        OneKey::TYPE_ID_JSON,
        None,
        vec![
            (email_token, email.value().into_encoded_scalar()),
            (email_token, email.value().into_encoded_scalar()),
        ],
    ));
    assert_eq!((err.code(), err.field()), ("duplicate_reference_key", "participant[0].email"));

    let err = player_error(HydratedPlayer::new(
        NoKey::TYPE_ID_JSON,
        Some("no-key-iid".to_owned()),
        vec![(email_token, email.value().into_encoded_scalar())],
    ));
    assert_eq!((err.code(), err.field()), ("unexpected_reference_key", "participant[0].keys[0]"));

    let err = player_error(HydratedPlayer::new(
        OneKey::TYPE_ID_JSON,
        Some("one-key-iid".to_owned()),
        vec![(external_token, external_id.value().into_encoded_scalar())],
    ));
    assert_eq!((err.code(), err.field()), ("unexpected_reference_key", "participant[0].keys[0]"));

    let err = player_error(HydratedPlayer::new(
        MultiKey::TYPE_ID_JSON,
        None,
        vec![
            (MultiKeyType::email.owns_id_json(), email.value().into_encoded_scalar()),
            (
                MultiKeyType::external_id.owns_id_json(),
                external_id.value().into_encoded_scalar(),
            ),
        ],
    ));
    assert_eq!(
        (err.code(), err.field()),
        ("multiple_reference_keys_without_iid", "participant[0]")
    );

    let err = player_error(HydratedPlayer::new(
        OneKey::TYPE_ID_JSON,
        None,
        vec![(email_token, EncodedScalar::Long(7))],
    ));
    assert_eq!((err.code(), err.field()), ("wrong_scalar_domain", "participant[0].email"));

    let success = HydratedPlayer::new(
        MultiKey::TYPE_ID_JSON,
        Some("multi-player-iid".to_owned()),
        vec![
            (MultiKeyType::email.owns_id_json(), email.value().into_encoded_scalar()),
            (
                MultiKeyType::external_id.owns_id_json(),
                external_id.value().into_encoded_scalar(),
            ),
        ],
    );
    let row = HydratedRow::new(
        ReferenceHolder::TYPE_ID_JSON,
        "holder-success".to_owned(),
        vec![],
        vec![(
            ReferenceHolderType::participant.role_id_json(),
            vec![success],
        )],
    );
    let holder: ReferenceHolder = materialize_model_for_test(&row)?;
    assert_eq!(holder.participant().len(), 1);

    let _single = SingleLinkCreate::new(OneKeyRef::from_iid("one-iid")?)?;
    let _mixed = MixedLinkCreate::new(MixedLinkParticipantRef::NoKey(
        NoKeyRef::from_iid("no-iid")?,
    ))?;

    println!("06B ERROR missing_reference_identity participant[0]");
    println!("06B ERROR empty_iid participant[0].iid");
    println!("06B ERROR duplicate_reference_key participant[0].email");
    println!("06B ERROR unexpected_reference_key participant[0].keys[0]");
    println!("06B ERROR multiple_reference_keys_without_iid participant[0]");
    println!("06B ERROR wrong_scalar_domain participant[0].email");
    println!("Review 06B reference/family probes PASSED.");
    Ok(())
}
"#;
    write_consumer_with_features(&consumer, "consumer-06b", main_rs, &["test-harness"]);
    let output = cargo(
        &[
            "run",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("consumer-06b-target"),
    );
    assert!(
        output.status.success(),
        "Review 06B consumer failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));

    let no_key_negative = stage.path().join("no-key-negative");
    write_consumer_with_features(
        &no_key_negative,
        "no-key-negative",
        "use generated::*;\nfn main() { let _ = NoKeyRef::from_key(Email::new(\"x\").unwrap()); }\n",
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--manifest-path",
            no_key_negative.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("no-key-negative-target"),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NoKeyRef"));
    assert!(stderr.contains("from_key"));
    assert!(
        stderr.contains("not found") || stderr.contains("no function or associated item"),
        "no-key constructor failure was not the intended missing item:\n{stderr}"
    );

    let family_negative = stage.path().join("family-negative");
    write_consumer_with_features(
        &family_negative,
        "family-negative",
        "use generated::*;\nfn probe(value: &FamilyRootFamily) { let _ = value.redeclared_value(); }\nfn main() {}\n",
        &[],
    );
    let output = cargo(
        &[
            "check",
            "--manifest-path",
            family_negative.join("Cargo.toml").to_str().unwrap(),
        ],
        &stage.path().join("family-negative-target"),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("FamilyRootFamily"));
    assert!(stderr.contains("redeclared_value"));
    assert!(stderr.contains("no method"));
}
