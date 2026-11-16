//! Internal optional-pattern lowering; public SDK builders do not expose it.
use std::env;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    Operation, Pattern, PlanSpec, QueryOperand, QueryOutput, QueryPattern, QueryPlan, Stage,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::staged::{PlanAnswer, TransactionOptions};
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct FixturePlan {
    bindings: Vec<AssertionBinding>,
    patterns: Vec<QueryPattern>,
}
impl FixturePlan {
    fn binding(&mut self, name: &str) -> BindingId {
        let id = BindingId::new(self.bindings.len().try_into().unwrap()).unwrap();
        self.bindings
            .push(AssertionBinding::new(id, QueryVariable::new(name).unwrap()));
        id
    }
    fn thing(&mut self, name: &str, kind: TypeKind, label: &str) -> BindingId {
        let binding = self.binding(name);
        self.patterns.push(QueryPattern::Isa {
            binding,
            include_subtypes: label == "person",
            type_id: TypeId::new(kind, label).unwrap(),
        });
        binding
    }
    fn field(&mut self, owner: BindingId, name: &str, label: &str, optional: bool) -> BindingId {
        let attribute = self.binding(name);
        let patterns = vec![QueryPattern::Has {
            owner,
            attribute,
            attribute_id: AttributeId::new(label).unwrap(),
        }];
        if optional {
            self.patterns.push(QueryPattern::Try { patterns });
        } else {
            self.patterns.extend(patterns);
        }
        attribute
    }
    fn link(&mut self, relation: BindingId, relation_label: &str, role: &str, player: BindingId) {
        self.patterns.push(QueryPattern::Links {
            relation,
            relation_id: TypeId::new(TypeKind::Relation, relation_label).unwrap(),
            players: vec![AssertionRolePlayer::new(
                RoleId::new(relation_label, role).unwrap(),
                player,
            )],
        });
    }
}
fn equal(binding: BindingId, text: &str) -> QueryPattern {
    QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Binding { binding },
        right: QueryOperand::Literal {
            value: CanonicalValue::String(CanonicalString::new(text).unwrap()),
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let width: usize = env::var("TYPE_BRIDGE_COMPLEX_WIDTH")?.parse()?;
    assert!([4, 10].contains(&width));
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("schema.yaml")?, include_str!("schema.yaml"))])?;
    let declared = normalize_documents(&documents)?;
    let mut fixture = FixturePlan {
        bindings: vec![],
        patterns: vec![],
    };
    let types = [
        ("p", TypeKind::Entity, "person"),
        ("e", TypeKind::Relation, "employment"),
        ("c", TypeKind::Entity, "company"),
        ("m", TypeKind::Relation, "membership"),
        ("d", TypeKind::Entity, "department"),
        ("l", TypeKind::Relation, "located"),
        ("o", TypeKind::Entity, "office"),
        ("z", TypeKind::Relation, "company-office"),
        ("a", TypeKind::Relation, "assignment"),
        ("j", TypeKind::Entity, "project"),
    ];
    let things = types[..width]
        .iter()
        .map(|(name, kind, label)| fixture.thing(name, *kind, label))
        .collect::<Vec<_>>();
    for (relation, label, role, player) in [
        (1, "employment", "worker", 0),
        (1, "employment", "employer", 2),
        (3, "membership", "member", 0),
        (3, "membership", "department", 4),
        (5, "located", "department", 4),
        (5, "located", "office", 6),
        (7, "company-office", "company", 2),
        (7, "company-office", "office", 6),
        (8, "assignment", "worker", 0),
        (8, "assignment", "project", 9),
    ] {
        if relation < width && player < width {
            fixture.link(things[relation], label, role, things[player]);
        }
    }
    let identifier = fixture.field(things[0], "identifier", "identifier", false);
    let note = fixture.field(things[1], "note", "note", false);
    let nickname = fixture.field(things[0], "nickname", "nickname", true);
    fixture.patterns.push(QueryPattern::Or {
        branches: vec![
            vec![equal(identifier, "P0002")],
            vec![equal(identifier, "P0004")],
            vec![equal(identifier, "P0008")],
        ],
    });
    fixture.patterns.push(QueryPattern::Value {
        comparator: ValueComparator::Like,
        left: QueryOperand::Binding { binding: note },
        right: QueryOperand::Literal {
            value: CanonicalValue::String(CanonicalString::new("^job-.*$")?),
        },
    });
    for (name, tag) in [("tag_a", "a"), ("tag_z", "z")] {
        let tag_binding = fixture.field(things[0], name, "tag", false);
        fixture.patterns.push(equal(tag_binding, tag));
    }
    let score = fixture.field(things[0], "score", "score", false);
    fixture.patterns.push(QueryPattern::Value {
        comparator: ValueComparator::GreaterOrEqual,
        left: QueryOperand::Binding { binding: score },
        right: QueryOperand::Literal {
            value: CanonicalValue::Long(20),
        },
    });
    let company_id = fixture.field(things[2], "company_id", "identifier", false);
    fixture.patterns.push(QueryPattern::Value {
        comparator: ValueComparator::StartsWith,
        left: QueryOperand::Binding {
            binding: company_id,
        },
        right: QueryOperand::Literal {
            value: CanonicalValue::String(CanonicalString::new("C")?),
        },
    });
    if width == 10 {
        let blocked = fixture.binding("blocked");
        fixture.patterns.push(QueryPattern::Not {
            patterns: vec![
                QueryPattern::Has {
                    owner: things[4],
                    attribute: blocked,
                    attribute_id: AttributeId::new("blocked")?,
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: blocked },
                    right: QueryOperand::Literal {
                        value: CanonicalValue::Boolean(true),
                    },
                },
            ],
        });
        let site = fixture.field(things[6], "site", "site", false);
        fixture.patterns.push(QueryPattern::Value {
            comparator: ValueComparator::Contains,
            left: QueryOperand::Binding { binding: site },
            right: QueryOperand::Literal {
                value: CanonicalValue::String(CanonicalString::new("north")?),
            },
        });
        let priority = fixture.field(things[9], "priority", "priority", false);
        fixture.patterns.push(QueryPattern::Value {
            comparator: ValueComparator::GreaterOrEqual,
            left: QueryOperand::Binding { binding: score },
            right: QueryOperand::Binding { binding: priority },
        });
        fixture.patterns.push(QueryPattern::Value {
            comparator: ValueComparator::GreaterOrEqual,
            left: QueryOperand::Binding { binding: priority },
            right: QueryOperand::Literal {
                value: CanonicalValue::Long(1),
            },
        });
    }
    let profile = SemanticProfileId::new(
        if env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")?.starts_with("3.12.") {
            "typedb-3.12.1/v1"
        } else {
            "typedb-3.13.6/v1"
        },
    )?;
    let plan = QueryPlan::new(
        PlanSpec {
            bindings: fixture.bindings,
            inputs: vec![],
            given_rows: None,
            operation: Operation::Rows,
            pipeline: vec![Stage::Match {
                patterns: fixture.patterns.into_iter().map(Pattern::Read).collect(),
            }],
            output: QueryOutput::Rows {
                columns: vec![identifier, note, nickname],
            },
            managed_semantics: ManagedSemanticSchemaFingerprint::compute(
                profile,
                b"complex-fixture",
            )?,
            functions: vec![],
        },
        &declared,
    )?;
    let db = Database::connect_with_options(
        &env::var("TYPEDB_ADDRESS")?,
        &env::var("TYPE_BRIDGE_COMPLEX_DATABASE")?,
        "admin",
        "password",
        ConnectOptions {
            http_port: env::var("TYPEDB_HTTP_PORT")?.parse()?,
            tls: false,
            server_version: Some(env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")?.parse()?),
        },
    )
    .await?;
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan)?;
    tx.commit().await?;
    let answer = pending.get()?;
    let PlanAnswer::Rows(rows) = answer.as_ref() else {
        panic!("rows expected")
    };
    println!("{}", serde_json::to_string(rows)?);
    db.close()?;
    Ok(())
}
