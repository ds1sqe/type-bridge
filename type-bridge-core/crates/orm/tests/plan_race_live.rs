//! The concurrency guide's races as staged `query-plan/v3` programs, live on
//! TypeDB 3.13.6.
//!
//! Each race stages one plan program in each of two transactions and holds
//! both provider commits until both have run, so the transactions overlap.
//! The outcomes are the guide's: the first committer wins, a conflicting
//! loser is a typed `Rejected { Commit, Conflict { Exclusive } }` (`STC2`),
//! and the winner is never assumed.
#![cfg(feature = "typedb")]

#[path = "../../../../tests/support/commit_gate.rs"]
mod commit_gate;

use std::time::Duration;

use commit_gate::{CommitGate, GatedBackend};
use type_bridge_contract::failure::{Failure, LockKind, Phase, Reason};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::v3::{
    ArithOp, Expr, InsertThing, Operation, Pattern, PlanSpec, QueryPlanV3, Set, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_orm::session::backend::{QueryResult, TxType};
use type_bridge_orm::session::real_driver::RealBackend;
use type_bridge_orm::session::staged::{CommitError, PlanAnswer, TransactionOptions};
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_schema_compat::typeql_to_declared;

const GUIDE: &str = include_str!("../../../../docs/guide/concurrency.md");

/// One named TypeQL fence from the guide.
fn snippet(name: &str) -> &'static str {
    let fence = format!("```typeql title=\"{name}\"\n");
    let (_, rest) = GUIDE
        .split_once(&fence)
        .unwrap_or_else(|| panic!("missing guide snippet {name}"));
    rest.split_once("\n```").expect("closed TypeQL fence").0
}

/// A plan program under construction over the guide's schema.
struct Plan {
    names: Vec<&'static str>,
    inputs: Vec<InputColumn>,
    row: Vec<Option<CanonicalValue>>,
    stages: Vec<Stage>,
}

impl Plan {
    fn new(names: &[&'static str]) -> Self {
        Self {
            names: names.to_vec(),
            inputs: Vec::new(),
            row: Vec::new(),
            stages: Vec::new(),
        }
    }

    fn at(&self, name: &str) -> BindingId {
        let index = self
            .names
            .iter()
            .position(|candidate| *candidate == name)
            .unwrap_or_else(|| panic!("undeclared binding {name}"));
        BindingId::new(u16::try_from(index).unwrap()).unwrap()
    }

    fn var(&self, name: &str) -> QueryOperand {
        QueryOperand::Binding {
            binding: self.at(name),
        }
    }

    /// A caller value, carried as a `given` input.
    fn input(&mut self, name: &str, value: &str) -> QueryOperand {
        let column = InputColumnId::new(u16::try_from(self.inputs.len()).unwrap());
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(name).unwrap(),
            ValueTypeTag::String,
            false,
        ));
        self.row.push(Some(text(value)));
        QueryOperand::Input { column }
    }

    fn isa(&self, name: &str, label: &str) -> Pattern {
        QueryPattern::Isa {
            binding: self.at(name),
            include_subtypes: false,
            type_id: TypeId::new(kind(label), label).unwrap(),
        }
        .into()
    }

    fn has(&self, owner: &str, value: &str, attribute: &str) -> Pattern {
        QueryPattern::Has {
            owner: self.at(owner),
            attribute: self.at(value),
            attribute_id: AttributeId::new(attribute).unwrap(),
        }
        .into()
    }

    fn compare(comparator: ValueComparator, left: QueryOperand, right: QueryOperand) -> Pattern {
        QueryPattern::Value {
            comparator,
            left,
            right,
        }
        .into()
    }

    fn set(&self, owner: &str, owner_type: &str, attribute: &str, operand: QueryOperand) -> Set {
        Set::Field {
            owner: self.at(owner),
            field: owns(owner_type, attribute),
            operand,
        }
    }

    fn build(self, schema: &DeclaredSchema) -> QueryPlanV3 {
        let given_rows = (!self.inputs.is_empty()).then(|| vec![InputRow::new(self.row)]);
        QueryPlanV3::new(
            PlanSpec {
                bindings: self
                    .names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        AssertionBinding::new(
                            BindingId::new(u16::try_from(index).unwrap()).unwrap(),
                            QueryVariable::new(*name).unwrap(),
                        )
                    })
                    .collect(),
                inputs: self.inputs,
                given_rows,
                operation: Operation::Write {
                    returns: WriteReturns::Matched,
                },
                pipeline: self.stages,
                output: QueryOutput::Rows { columns: vec![] },
                managed_semantics: ManagedSemanticSchemaFingerprint::compute(
                    SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                    snippet("schema").as_bytes(),
                )
                .unwrap(),
                compatibility: None,
                functions: vec![],
            },
            schema,
        )
        .unwrap()
    }
}

fn kind(label: &str) -> TypeKind {
    match label {
        "purchase" | "assignment" => TypeKind::Relation,
        _ => TypeKind::Entity,
    }
}

fn owns(owner: &str, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(
        TypeId::new(kind(owner), owner).unwrap(),
        AttributeId::new(attribute).unwrap(),
    )
    .unwrap()
}

fn text(value: &str) -> CanonicalValue {
    CanonicalValue::String(CanonicalString::new(value).unwrap())
}

fn long(value: i64) -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::Long(value),
    }
}

fn sub(left: QueryOperand, right: QueryOperand) -> Expr {
    Expr::Arith {
        op: ArithOp::Sub,
        lhs: Box::new(Expr::Operand(left)),
        rhs: Box::new(Expr::Operand(right)),
    }
}

/// The guide's "purchase": check stock and balance, take one item and the
/// price, and record the purchase in one pipeline.
fn purchase(schema: &DeclaredSchema, purchase_id: &str) -> QueryPlanV3 {
    let mut plan = Plan::new(&[
        "item",
        "account",
        "item_key",
        "stock",
        "price",
        "account_key",
        "balance",
        "next_stock",
        "next_balance",
        "purchase",
    ]);
    let id = plan.input("purchase_id", purchase_id);
    let item_id = plan.input("item_id", "item-1");
    let account_id = plan.input("account_id", "account-1");
    let patterns = vec![
        plan.isa("item", "item"),
        plan.isa("account", "account"),
        plan.has("item", "item_key", "item-id"),
        plan.has("item", "stock", "stock"),
        plan.has("item", "price", "price"),
        plan.has("account", "account_key", "account-id"),
        plan.has("account", "balance", "balance"),
        Plan::compare(ValueComparator::Equal, plan.var("item_key"), item_id),
        Plan::compare(ValueComparator::Equal, plan.var("account_key"), account_id),
        Plan::compare(ValueComparator::Greater, plan.var("stock"), long(0)),
        Plan::compare(
            ValueComparator::GreaterOrEqual,
            plan.var("balance"),
            plan.var("price"),
        ),
        Pattern::Let {
            binding: plan.at("next_stock"),
            expr: sub(plan.var("stock"), long(1)),
        },
        Pattern::Let {
            binding: plan.at("next_balance"),
            expr: sub(plan.var("balance"), plan.var("price")),
        },
    ];
    let sets = vec![
        plan.set("item", "item", "stock", plan.var("next_stock")),
        plan.set("account", "account", "balance", plan.var("next_balance")),
    ];
    let record = InsertThing {
        binding: plan.at("purchase"),
        type_id: TypeId::new(TypeKind::Relation, "purchase").unwrap(),
        fields: vec![(owns("purchase", "purchase-id"), id)],
        players: vec![
            (
                RoleId::new("purchase", "buyer").unwrap(),
                plan.at("account"),
            ),
            (RoleId::new("purchase", "item").unwrap(), plan.at("item")),
        ],
    };
    plan.stages = vec![
        Stage::Match { patterns },
        Stage::Update { sets },
        Stage::Insert {
            things: vec![record],
        },
    ];
    plan.build(schema)
}

/// The guide's "keyed-purchase": record a purchase under one key.
fn keyed_purchase(schema: &DeclaredSchema, purchase_id: &str) -> QueryPlanV3 {
    let mut plan = Plan::new(&["item", "account", "item_key", "account_key", "purchase"]);
    let id = plan.input("purchase_id", purchase_id);
    let item_id = plan.input("item_id", "item-1");
    let account_id = plan.input("account_id", "account-1");
    let patterns = vec![
        plan.isa("item", "item"),
        plan.isa("account", "account"),
        plan.has("item", "item_key", "item-id"),
        plan.has("account", "account_key", "account-id"),
        Plan::compare(ValueComparator::Equal, plan.var("item_key"), item_id),
        Plan::compare(ValueComparator::Equal, plan.var("account_key"), account_id),
    ];
    let record = InsertThing {
        binding: plan.at("purchase"),
        type_id: TypeId::new(TypeKind::Relation, "purchase").unwrap(),
        fields: vec![(owns("purchase", "purchase-id"), id)],
        players: vec![
            (
                RoleId::new("purchase", "buyer").unwrap(),
                plan.at("account"),
            ),
            (RoleId::new("purchase", "item").unwrap(), plan.at("item")),
        ],
    };
    plan.stages = vec![
        Stage::Match { patterns },
        Stage::Insert {
            things: vec![record],
        },
    ];
    plan.build(schema)
}

/// The guide's "doctor-a-off" / "doctor-b-off".
fn doctor_off(schema: &DeclaredSchema, name: &str) -> QueryPlanV3 {
    let mut plan = Plan::new(&["doctor", "name"]);
    let doctor = plan.input("doctor_name", name);
    let patterns = vec![
        plan.isa("doctor", "doctor"),
        plan.has("doctor", "name", "name"),
        Plan::compare(ValueComparator::Equal, plan.var("name"), doctor),
    ];
    let off = QueryOperand::Literal {
        value: CanonicalValue::Boolean(false),
    };
    plan.stages = vec![
        Stage::Match { patterns },
        Stage::Update {
            sets: vec![plan.set("doctor", "doctor", "on-call", off)],
        },
    ];
    plan.build(schema)
}

/// The guide's "touch-on-call": rewrite every on-call value with itself.
fn touch(schema: &DeclaredSchema) -> QueryPlanV3 {
    let mut plan = Plan::new(&["doctor", "value"]);
    plan.stages = vec![
        Stage::Match {
            patterns: vec![
                plan.isa("doctor", "doctor"),
                plan.has("doctor", "value", "on-call"),
            ],
        },
        Stage::Update {
            sets: vec![plan.set("doctor", "doctor", "on-call", plan.var("value"))],
        },
    ];
    plan.build(schema)
}

/// The guide's "assign-first-shift" / "assign-second-shift".
fn assign(schema: &DeclaredSchema, shift_id: &str) -> QueryPlanV3 {
    let mut plan = Plan::new(&["doctor", "name", "shift", "shift_key", "assignment"]);
    let doctor = plan.input("doctor_name", "doctor-a");
    let shift = plan.input("shift_id", shift_id);
    let patterns = vec![
        plan.isa("doctor", "doctor"),
        plan.has("doctor", "name", "name"),
        plan.isa("shift", "shift"),
        plan.has("shift", "shift_key", "shift-id"),
        Plan::compare(ValueComparator::Equal, plan.var("name"), doctor),
        Plan::compare(ValueComparator::Equal, plan.var("shift_key"), shift),
    ];
    let assignment = InsertThing {
        binding: plan.at("assignment"),
        type_id: TypeId::new(TypeKind::Relation, "assignment").unwrap(),
        fields: vec![],
        players: vec![
            (
                RoleId::new("assignment", "assignee").unwrap(),
                plan.at("doctor"),
            ),
            (
                RoleId::new("assignment", "target").unwrap(),
                plan.at("shift"),
            ),
        ],
    };
    plan.stages = vec![
        Stage::Match { patterns },
        Stage::Insert {
            things: vec![assignment],
        },
    ];
    plan.build(schema)
}

type Outcome = Result<Vec<PlanAnswer>, CommitError>;

struct Live {
    backend: RealBackend,
    databases: Vec<Database>,
}

impl Live {
    /// A fresh database seeded from the guide, plus a gated handle to it.
    async fn recipe(&mut self, recipe: &str) -> (Database, Database, CommitGate) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("tb_w11e_race_{recipe}_{}_{nonce}", std::process::id());
        let plain = Database::with_backend(Box::new(self.backend.clone()), &name);
        plain.create_database().await.unwrap();
        plain
            .execute_raw(snippet("schema"), TxType::Schema)
            .await
            .unwrap();
        plain
            .execute_raw(snippet("seed"), TxType::Write)
            .await
            .unwrap();
        let gate = CommitGate::default();
        let gated = Database::with_backend(
            Box::new(GatedBackend::new(self.backend.clone(), gate.clone())),
            &name,
        );
        self.databases.push(Database::with_backend(
            Box::new(self.backend.clone()),
            &name,
        ));
        (plain, gated, gate)
    }
}

/// Stage `plans` in one transaction and commit it.
async fn run(database: &Database, plans: &[QueryPlanV3]) -> Outcome {
    let mut transaction = database.transaction(TransactionOptions::default());
    let answers = plans
        .iter()
        .map(|plan| transaction.stage_plan(plan).unwrap())
        .collect::<Vec<_>>();
    transaction.commit().await?;
    Ok(answers
        .iter()
        .map(|answer| (*answer.get().unwrap()).clone())
        .collect())
}

/// Commit both programs concurrently; neither snapshot sees the other.
async fn race(
    gated: &Database,
    gate: &CommitGate,
    a: &[QueryPlanV3],
    b: &[QueryPlanV3],
) -> [Outcome; 2] {
    gate.arm(2);
    let (a, b) = tokio::time::timeout(Duration::from_secs(120), async {
        tokio::join!(run(gated, a), run(gated, b))
    })
    .await
    .expect("both racers reach their commits");
    [a, b]
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Ok(answers) => format!("committed {answers:?}"),
        Err(error) => describe_error(error),
    }
}

/// The canonical diagnostic code and the typed failure, never provider text.
fn describe_error(error: &CommitError) -> String {
    format!(
        "{} {}",
        error.diagnostic().code().as_str(),
        error.failure.as_ref().map_or_else(
            || "unclassified".to_owned(),
            |failure| failure.host_json().to_string()
        )
    )
}

/// Exactly one racer commits; the other is the guide's `STC2` loser.
/// Returns the winner's answers.
fn first_committer_wins(scenario: &str, outcomes: [Outcome; 2]) -> Vec<PlanAnswer> {
    println!(
        "{scenario}: racer A {}; racer B {}",
        describe(&outcomes[0]),
        describe(&outcomes[1])
    );
    let [a, b] = outcomes;
    let (winner, loser) = match (a, b) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        (a, b) => panic!(
            "{scenario}: exactly one racer must commit: {} / {}",
            describe(&a),
            describe(&b)
        ),
    };
    assert!(
        matches!(
            loser.failure.as_deref(),
            Some(Failure::Rejected {
                phase: Phase::Commit,
                reason: Reason::Conflict {
                    lock: LockKind::Exclusive
                },
                ..
            })
        ),
        "{scenario}: the loser is a commit-time exclusive-lock conflict: {}",
        describe_error(&loser)
    );
    assert_eq!(
        loser.failure.as_deref().and_then(Failure::code),
        Some("STC2")
    );
    winner
}

async fn rows(database: &Database, query: &str) -> usize {
    match database.execute_raw(query, TxType::Read).await.unwrap() {
        QueryResult::Rows(rows) | QueryResult::Documents(rows) => rows.len(),
        QueryResult::Ok => panic!("expected rows for {query}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated TypeDB 3.13.6 server (TYPEDB_ADDRESS, TYPEDB_HTTP_PORT)"]
async fn staged_plans_race_like_the_concurrency_guide_on_3_13_6() {
    let address = std::env::var("TYPEDB_ADDRESS").expect("TYPEDB_ADDRESS");
    let backend = RealBackend::connect(
        &address,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .expect("TYPEDB_HTTP_PORT")
                .parse()
                .expect("valid TypeDB HTTP port"),
            ..ConnectOptions::default()
        },
    )
    .await
    .unwrap();
    let schema = typeql_to_declared(
        DocumentId::new("concurrency-guide-schema.typeql").unwrap(),
        snippet("schema"),
    )
    .unwrap();
    let mut live = Live {
        backend,
        databases: Vec::new(),
    };
    assert_eq!(
        Database::with_backend(Box::new(live.backend.clone()), "version-probe")
            .server_version()
            .map(|version| version.to_string()),
        Some("3.13.6".to_owned()),
        "the guide's outcomes are pinned to TypeDB 3.13.6"
    );

    // Purchase: both buyers see the last item; the stock and balance
    // ownerships' default @card(0..1) locks let only one commit.
    let (plain, gated, gate) = live.recipe("purchase").await;
    let winner = first_committer_wins(
        "purchase",
        race(
            &gated,
            &gate,
            &[purchase(&schema, "purchase-1")],
            &[purchase(&schema, "purchase-2")],
        )
        .await,
    );
    assert_eq!(winner, [PlanAnswer::Matched(1)]);
    assert_eq!(
        rows(&plain, "match $i isa item, has stock 0; select $i;").await,
        1
    );
    assert_eq!(
        rows(&plain, "match $a isa account, has balance 60; select $a;").await,
        1
    );
    assert_eq!(rows(&plain, "match $p isa purchase; select $p;").await, 1);
    // With the stock gone the same pipeline matches nothing and writes
    // nothing: a committed condition failure, never a negative stock.
    assert_eq!(
        run(&plain, &[purchase(&schema, "purchase-3")])
            .await
            .unwrap(),
        [PlanAnswer::Matched(0)]
    );
    assert_eq!(
        rows(
            &plain,
            "match $i isa item, has stock $s; $s < 0; select $i;"
        )
        .await,
        0
    );
    assert_eq!(rows(&plain, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(
        rows(&plain, "match $a isa account, has balance 60; select $a;").await,
        1
    );

    // Write skew: different owners, no shared write, both commit.
    let (plain, gated, gate) = live.recipe("skew").await;
    let outcomes = race(
        &gated,
        &gate,
        &[doctor_off(&schema, "doctor-a")],
        &[doctor_off(&schema, "doctor-b")],
    )
    .await;
    println!(
        "write skew: racer A {}; racer B {}",
        describe(&outcomes[0]),
        describe(&outcomes[1])
    );
    for outcome in outcomes {
        assert_eq!(outcome.unwrap(), [PlanAnswer::Matched(1)]);
    }
    assert_eq!(
        rows(&plain, "match $d isa doctor, has on-call true; select $d;").await,
        0
    );

    // A same-value touch of both on-call ownerships makes them conflict.
    let (plain, gated, gate) = live.recipe("touch").await;
    let winner = first_committer_wins(
        "touch",
        race(
            &gated,
            &gate,
            &[touch(&schema), doctor_off(&schema, "doctor-a")],
            &[touch(&schema), doctor_off(&schema, "doctor-b")],
        )
        .await,
    );
    assert_eq!(winner, [PlanAnswer::Matched(2), PlanAnswer::Matched(1)]);
    assert_eq!(
        rows(&plain, "match $d isa doctor, has on-call true; select $d;").await,
        1
    );

    // @card(0..1) on plays: one assignment per doctor.
    let (plain, gated, gate) = live.recipe("plays").await;
    let winner = first_committer_wins(
        "plays @card",
        race(
            &gated,
            &gate,
            &[assign(&schema, "shift-1")],
            &[assign(&schema, "shift-2")],
        )
        .await,
    );
    assert_eq!(winner, [PlanAnswer::Matched(1)]);
    assert_eq!(rows(&plain, "match $a isa assignment; select $a;").await, 1);
    // Once the winner is visible, another assignment fails the cardinality
    // check at commit (CNT5).
    let late = run(&plain, &[assign(&schema, "shift-2")])
        .await
        .expect_err("a visible assignment exhausts the plays cardinality");
    println!("plays @card after the winner: {}", describe_error(&late));
    assert!(
        matches!(
            late.failure.as_deref(),
            Some(Failure::Rejected {
                phase: Phase::Commit,
                reason: Reason::Cardinality,
                ..
            })
        ),
        "{}",
        describe_error(&late)
    );
    assert_eq!(rows(&plain, "match $a isa assignment; select $a;").await, 1);

    // @key: two inserts of one purchase key; one commits.
    let (plain, gated, gate) = live.recipe("key").await;
    let winner = first_committer_wins(
        "@key",
        race(
            &gated,
            &gate,
            &[keyed_purchase(&schema, "purchase-key")],
            &[keyed_purchase(&schema, "purchase-key")],
        )
        .await,
    );
    assert_eq!(winner, [PlanAnswer::Matched(1)]);
    // Once the key is visible, the duplicate fails in query execution (CNT9).
    let duplicate = run(&plain, &[keyed_purchase(&schema, "purchase-key")])
        .await
        .expect_err("a visible key rejects the duplicate");
    println!("@key after the winner: {}", describe_error(&duplicate));
    assert!(
        matches!(
            duplicate.failure.as_deref(),
            Some(Failure::Rejected {
                phase: Phase::Query,
                reason: Reason::Unique,
                op_index: Some(0),
            })
        ),
        "{}",
        describe_error(&duplicate)
    );
    assert_eq!(rows(&plain, "match $p isa purchase; select $p;").await, 1);

    for database in live.databases {
        database.delete_database().await.unwrap();
    }
}
