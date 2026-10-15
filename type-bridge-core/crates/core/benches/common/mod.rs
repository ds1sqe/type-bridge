//! Bench inputs shared by the Criterion (wall time) and gungraun (instruction
//! count) benches, so both measure the same work under the same ids.
// Each bench binary compiles this module but uses only its own fixtures.
#![allow(dead_code)]
#![allow(deprecated)] // Legacy node bridges are removed in 11.30 / 3.0.

use serde_json::json;
use type_bridge_core_lib::ast::{
    ArithmeticValue, Clause, Constraint, FetchItem, FunctionCallValue, LetAssignment, LiteralValue,
    Pattern, ReduceAssignment, RolePlayer, Statement, Value,
};

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn long_type_name() -> String {
    "a".repeat(100) + "-type"
}

/// `1000 * scale` type names: plain, long, relation-style and non-ASCII.
pub fn batch_type_names(scale: usize) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(1000 * scale);
    for i in 0..400 * scale {
        names.push(format!("entity-type-{}", i));
    }
    for i in 0..300 * scale {
        names.push(format!("my-long-attribute-name-for-testing-{}", i));
    }
    for i in 0..200 * scale {
        names.push(format!("relation-{}-data", i));
    }
    for i in 0..100 * scale {
        names.push(format!("\u{00e9}l\u{00e8}ve-{}", i));
    }
    names
}

pub fn simple_entity_pattern() -> Pattern {
    Pattern::Entity {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![Constraint::Has {
            attr_name: "name".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!("Alice"),
                value_type: "string".to_string(),
            }),
        }],
        is_strict: false,
    }
}

pub fn complex_nested_pattern() -> Pattern {
    Pattern::Or(vec![
        vec![
            Pattern::Entity {
                variable: "$p".to_string(),
                type_name: "person".to_string(),
                constraints: vec![
                    Constraint::Has {
                        attr_name: "name".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!("Alice"),
                            value_type: "string".to_string(),
                        }),
                    },
                    Constraint::Has {
                        attr_name: "age".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!(30),
                            value_type: "long".to_string(),
                        }),
                    },
                ],
                is_strict: false,
            },
            Pattern::Relation {
                variable: "$r".to_string(),
                type_name: "employment".to_string(),
                role_players: vec![
                    RolePlayer {
                        role: "employee".to_string(),
                        player_var: "$p".to_string(),
                    },
                    RolePlayer {
                        role: "employer".to_string(),
                        player_var: "$c".to_string(),
                    },
                ],
                constraints: vec![],
            },
        ],
        vec![Pattern::Not(vec![Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "retired-person".to_string(),
            constraints: vec![],
            is_strict: true,
        }])],
    ])
}

pub fn simple_isa_statement() -> Statement {
    Statement::Isa {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
    }
}

pub fn relation_with_attrs_statement() -> Statement {
    Statement::Relation {
        variable: "$rel".to_string(),
        type_name: "employment".to_string(),
        role_players: vec![
            RolePlayer {
                role: "employee".to_string(),
                player_var: "$p".to_string(),
            },
            RolePlayer {
                role: "employer".to_string(),
                player_var: "$c".to_string(),
            },
        ],
        include_variable: true,
        attributes: vec![
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "start-date".to_string(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!("2024-01-15"),
                    value_type: "date".to_string(),
                }),
            },
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "salary".to_string(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!(95000),
                    value_type: "long".to_string(),
                }),
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

pub const SMALL_SCHEMA: &str = r#"
define
attribute name, value string;
attribute email, value string;
attribute age, value long;
entity person, owns name @key, owns email @unique, owns age;
relation friendship, relates friend;
"#;

pub const MEDIUM_SCHEMA: &str = r#"
define
attribute name, value string;
attribute email, value string;
attribute age, value long;
attribute title, value string;
attribute isbn, value string;
attribute price, value double;
attribute rating, value double;
attribute published-date, value date;
attribute page-count, value long;
attribute description, value string;

entity base-entity @abstract, owns name;

entity person sub base-entity,
    owns email @key,
    owns age;

entity author sub person;
entity editor sub person;
entity reviewer sub person;

entity book,
    owns title,
    owns isbn @key,
    owns price,
    owns rating,
    owns published-date,
    owns page-count,
    owns description;

entity publisher, owns name @key;

relation authorship,
    relates written-by,
    relates written-work;

relation publishing,
    relates published-by,
    relates published-work;

relation review,
    relates reviewed-by,
    relates reviewed-work,
    relates review-score;
"#;

/// 120 types: 50 attributes, 50 entities and 20 relations.
pub fn large_schema() -> String {
    let mut tql = String::from("define\n");
    for i in 0..50 {
        tql.push_str(&format!("attribute attr-{}, value string;\n", i));
    }
    for i in 0..50 {
        tql.push_str(&format!(
            "entity entity-{}, owns attr-{} @key, owns attr-{};\n",
            i,
            i,
            (i + 1) % 50
        ));
    }
    for i in 0..20 {
        tql.push_str(&format!(
            "relation rel-{}, relates role-a, relates role-b;\n",
            i
        ));
    }
    tql
}

/// A ten-level `sub` chain below an abstract root.
pub fn inheritance_schema() -> String {
    let mut tql = String::from("define\n");
    tql.push_str("attribute name, value string;\n");
    tql.push_str("entity level-0 @abstract, owns name;\n");
    for i in 1..=10 {
        tql.push_str(&format!("attribute attr-{}, value string;\n", i));
        tql.push_str(&format!(
            "entity level-{} sub level-{}, owns attr-{};\n",
            i,
            i - 1,
            i
        ));
    }
    tql
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

pub fn make_simple_match() -> Clause {
    Clause::Match(vec![Pattern::Entity {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![],
        is_strict: false,
    }])
}

pub fn make_match_with_constraints() -> Clause {
    Clause::Match(vec![Pattern::Entity {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![
            Constraint::Has {
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".to_string(),
                }),
            },
            Constraint::Has {
                attr_name: "age".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(30),
                    value_type: "long".to_string(),
                }),
            },
            Constraint::Iid("0x1234567890abcdef".to_string()),
        ],
        is_strict: false,
    }])
}

pub fn make_complex_query() -> Clause {
    Clause::Match(vec![
        Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![
                Constraint::Has {
                    attr_name: "name".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!("Alice"),
                        value_type: "string".to_string(),
                    }),
                },
                Constraint::Has {
                    attr_name: "age".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!(30),
                        value_type: "long".to_string(),
                    }),
                },
            ],
            is_strict: false,
        },
        Pattern::Relation {
            variable: "$r".to_string(),
            type_name: "employment".to_string(),
            role_players: vec![
                RolePlayer {
                    role: "employee".to_string(),
                    player_var: "$p".to_string(),
                },
                RolePlayer {
                    role: "employer".to_string(),
                    player_var: "$c".to_string(),
                },
            ],
            constraints: vec![],
        },
        Pattern::Entity {
            variable: "$c".to_string(),
            type_name: "company".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "sector".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("tech"),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        },
        Pattern::Has {
            thing_var: "$p".to_string(),
            attr_type: "email".to_string(),
            attr_var: "$e".to_string(),
        },
        Pattern::ValueComparison {
            var: "$age".to_string(),
            operator: ">=".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(18),
                value_type: "long".to_string(),
            }),
        },
        Pattern::Not(vec![Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "retired-person".to_string(),
            constraints: vec![],
            is_strict: true,
        }]),
        Pattern::Or(vec![
            vec![Pattern::Has {
                thing_var: "$p".to_string(),
                attr_type: "status".to_string(),
                attr_var: "$s1".to_string(),
            }],
            vec![Pattern::Has {
                thing_var: "$p".to_string(),
                attr_type: "active".to_string(),
                attr_var: "$s2".to_string(),
            }],
        ]),
        Pattern::Entity {
            variable: "$d".to_string(),
            type_name: "department".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "budget".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(100000.0),
                    value_type: "double".to_string(),
                }),
            }],
            is_strict: false,
        },
        Pattern::Entity {
            variable: "$mgr".to_string(),
            type_name: "manager".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "level".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(3),
                    value_type: "long".to_string(),
                }),
            }],
            is_strict: false,
        },
        Pattern::Entity {
            variable: "$proj".to_string(),
            type_name: "project".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "deadline".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("2025-12-31"),
                    value_type: "date".to_string(),
                }),
            }],
            is_strict: false,
        },
    ])
}

pub fn make_batch_clauses() -> Vec<Clause> {
    let mut clauses = Vec::with_capacity(50);
    for i in 0..20 {
        clauses.push(Clause::Match(vec![Pattern::Entity {
            variable: format!("$e{}", i),
            type_name: format!("entity-type-{}", i),
            constraints: vec![Constraint::Has {
                attr_name: format!("attr-{}", i),
                value: Value::Literal(LiteralValue {
                    value: json!(format!("value-{}", i)),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        }]));
    }
    for i in 0..15 {
        clauses.push(Clause::Insert(vec![
            Statement::Isa {
                variable: format!("$n{}", i),
                type_name: format!("new-type-{}", i),
            },
            Statement::Has {
                subject_var: format!("$n{}", i),
                attr_name: format!("prop-{}", i),
                value: Value::Literal(LiteralValue {
                    value: json!(i * 10),
                    value_type: "long".to_string(),
                }),
            },
        ]));
    }
    for i in 0..10 {
        clauses.push(Clause::Delete(vec![Statement::DeleteThing(format!(
            "$d{}",
            i
        ))]));
    }
    for i in 0..5 {
        clauses.push(Clause::Fetch(vec![FetchItem::Attribute {
            key: format!("field-{}", i),
            var: format!("$f{}", i),
            attr_name: format!("data-{}", i),
        }]));
    }
    clauses
}

pub fn make_relation_insert() -> Clause {
    Clause::Insert(vec![Statement::Relation {
        variable: "$rel".to_string(),
        type_name: "employment".to_string(),
        role_players: vec![
            RolePlayer {
                role: "employee".to_string(),
                player_var: "$p".to_string(),
            },
            RolePlayer {
                role: "employer".to_string(),
                player_var: "$c".to_string(),
            },
            RolePlayer {
                role: "department".to_string(),
                player_var: "$d".to_string(),
            },
        ],
        include_variable: true,
        attributes: vec![
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "start-date".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("2024-01-15"),
                    value_type: "date".to_string(),
                }),
            },
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "salary".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(95000),
                    value_type: "long".to_string(),
                }),
            },
        ],
    }])
}

// ---------------------------------------------------------------------------
// Long query: 30 patterns (wide join)
// ---------------------------------------------------------------------------

pub fn make_long_query() -> Clause {
    let mut patterns = Vec::with_capacity(30);
    for i in 0..10 {
        patterns.push(Pattern::Entity {
            variable: format!("$e{}", i),
            type_name: format!("entity-type-{}", i),
            constraints: vec![
                Constraint::Has {
                    attr_name: format!("name-{}", i),
                    value: Value::Literal(LiteralValue {
                        value: json!(format!("val-{}", i)),
                        value_type: "string".to_string(),
                    }),
                },
                Constraint::Has {
                    attr_name: format!("count-{}", i),
                    value: Value::Literal(LiteralValue {
                        value: json!(i * 100),
                        value_type: "long".to_string(),
                    }),
                },
            ],
            is_strict: false,
        });
    }
    for i in 0..5 {
        patterns.push(Pattern::Relation {
            variable: format!("$r{}", i),
            type_name: format!("link-{}", i),
            role_players: vec![
                RolePlayer {
                    role: format!("source-{}", i),
                    player_var: format!("$e{}", i),
                },
                RolePlayer {
                    role: format!("target-{}", i),
                    player_var: format!("$e{}", i + 5),
                },
            ],
            constraints: vec![Constraint::Has {
                attr_name: format!("weight-{}", i),
                value: Value::Literal(LiteralValue {
                    value: json!(i as f64 * 0.5),
                    value_type: "double".to_string(),
                }),
            }],
        });
    }
    for i in 0..5 {
        patterns.push(Pattern::Has {
            thing_var: format!("$e{}", i),
            attr_type: format!("tag-{}", i),
            attr_var: format!("$t{}", i),
        });
    }
    for i in 0..5 {
        patterns.push(Pattern::ValueComparison {
            var: format!("$t{}", i),
            operator: ">=".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(format!("threshold-{}", i)),
                value_type: "string".to_string(),
            }),
        });
    }
    for i in 0..3 {
        patterns.push(Pattern::Iid {
            variable: format!("$x{}", i),
            iid: format!("0xabababababababab{:02x}", i),
        });
    }
    patterns.push(Pattern::Attribute {
        variable: "$salary".to_string(),
        type_name: "salary-amount".to_string(),
        value: Some(Value::Literal(LiteralValue {
            value: json!(50000),
            value_type: "long".to_string(),
        })),
    });
    patterns.push(Pattern::SubType {
        variable: "$t".to_string(),
        parent_type: "base-entity".to_string(),
    });
    Clause::Match(patterns)
}

// ---------------------------------------------------------------------------
// Deeply nested: or-of-not-of-or, 3 levels
// ---------------------------------------------------------------------------

pub fn make_deeply_nested() -> Clause {
    let inner_entities: Vec<Vec<Pattern>> = (0..4)
        .map(|i| {
            vec![Pattern::Entity {
                variable: format!("$deep{}", i),
                type_name: format!("leaf-type-{}", i),
                constraints: vec![Constraint::Has {
                    attr_name: format!("prop-{}", i),
                    value: Value::Literal(LiteralValue {
                        value: json!(format!("v{}", i)),
                        value_type: "string".to_string(),
                    }),
                }],
                is_strict: false,
            }]
        })
        .collect();
    let level3_or = Pattern::Or(inner_entities);

    let level2_blocks: Vec<Vec<Pattern>> = (0..3)
        .map(|i| {
            vec![
                Pattern::Not(vec![
                    level3_or.clone(),
                    Pattern::Entity {
                        variable: format!("$guard{}", i),
                        type_name: format!("guard-type-{}", i),
                        constraints: vec![Constraint::Has {
                            attr_name: "active".to_string(),
                            value: Value::Literal(LiteralValue {
                                value: json!(true),
                                value_type: "boolean".to_string(),
                            }),
                        }],
                        is_strict: false,
                    },
                ]),
                Pattern::Relation {
                    variable: format!("$rel{}", i),
                    type_name: format!("context-rel-{}", i),
                    role_players: vec![
                        RolePlayer {
                            role: "subject".to_string(),
                            player_var: format!("$guard{}", i),
                        },
                        RolePlayer {
                            role: "object".to_string(),
                            player_var: format!("$deep{}", i),
                        },
                    ],
                    constraints: vec![],
                },
            ]
        })
        .collect();

    let top_or = Pattern::Or(level2_blocks);

    Clause::Match(vec![
        Pattern::Entity {
            variable: "$root".to_string(),
            type_name: "root-entity".to_string(),
            constraints: vec![
                Constraint::Has {
                    attr_name: "name".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!("start"),
                        value_type: "string".to_string(),
                    }),
                },
                Constraint::Has {
                    attr_name: "priority".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!(1),
                        value_type: "long".to_string(),
                    }),
                },
                Constraint::Has {
                    attr_name: "score".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!(99.5),
                        value_type: "double".to_string(),
                    }),
                },
            ],
            is_strict: false,
        },
        top_or,
        Pattern::Not(vec![Pattern::Entity {
            variable: "$excluded".to_string(),
            type_name: "blacklisted".to_string(),
            constraints: vec![],
            is_strict: true,
        }]),
        Pattern::Has {
            thing_var: "$root".to_string(),
            attr_type: "timestamp".to_string(),
            attr_var: "$ts".to_string(),
        },
        Pattern::ValueComparison {
            var: "$ts".to_string(),
            operator: ">=".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!("2024-01-01"),
                value_type: "date".to_string(),
            }),
        },
    ])
}

// ---------------------------------------------------------------------------
// Graph traversal: 8-hop chain
// ---------------------------------------------------------------------------

pub fn make_graph_traversal() -> Clause {
    let hop_count = 8;
    let mut patterns = Vec::new();

    patterns.push(Pattern::Entity {
        variable: "$n0".to_string(),
        type_name: "person".to_string(),
        constraints: vec![
            Constraint::Has {
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".to_string(),
                }),
            },
            Constraint::Iid("0x0000000000000001".to_string()),
        ],
        is_strict: false,
    });

    for i in 0..hop_count {
        let rel_type = if i % 2 == 0 { "knows" } else { "works-with" };
        patterns.push(Pattern::Relation {
            variable: format!("$hop{}", i),
            type_name: rel_type.to_string(),
            role_players: vec![
                RolePlayer {
                    role: "from".to_string(),
                    player_var: format!("$n{}", i),
                },
                RolePlayer {
                    role: "to".to_string(),
                    player_var: format!("$n{}", i + 1),
                },
            ],
            constraints: vec![Constraint::Has {
                attr_name: "since".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(format!("20{}-01-01", 15 + i)),
                    value_type: "date".to_string(),
                }),
            }],
        });
        patterns.push(Pattern::Entity {
            variable: format!("$n{}", i + 1),
            type_name: "person".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "age".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(25 + i),
                    value_type: "long".to_string(),
                }),
            }],
            is_strict: false,
        });
    }

    for i in 0..=hop_count {
        patterns.push(Pattern::Has {
            thing_var: format!("$n{}", i),
            attr_type: "email".to_string(),
            attr_var: format!("$email{}", i),
        });
    }

    Clause::Match(patterns)
}

// ---------------------------------------------------------------------------
// Heavy insert: 100 entities x 6 attributes = 600 statements
// ---------------------------------------------------------------------------

pub fn make_heavy_insert() -> Clause {
    let mut statements = Vec::with_capacity(600);
    for i in 0..100 {
        let var = format!("$new{}", i);
        statements.push(Statement::Isa {
            variable: var.clone(),
            type_name: format!("data-record-{}", i % 10),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "name".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(format!("Record #{}", i)),
                value_type: "string".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "index".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(i),
                value_type: "long".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "score".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(i as f64 * 1.5),
                value_type: "double".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "active".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(i % 2 == 0),
                value_type: "boolean".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var,
            attr_name: "created-at".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(format!("2025-01-{:02}", (i % 28) + 1)),
                value_type: "date".to_string(),
            }),
        });
    }
    Clause::Insert(statements)
}

// ---------------------------------------------------------------------------
// Large batch: 200 mixed clauses
// ---------------------------------------------------------------------------

pub fn make_large_batch() -> Vec<Clause> {
    let mut clauses = Vec::with_capacity(200);
    for i in 0..80 {
        clauses.push(Clause::Match(vec![
            Pattern::Entity {
                variable: format!("$e{}", i),
                type_name: format!("type-{}", i % 20),
                constraints: vec![
                    Constraint::Has {
                        attr_name: "key".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!(format!("k-{}", i)),
                            value_type: "string".to_string(),
                        }),
                    },
                    Constraint::Has {
                        attr_name: "seq".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!(i),
                            value_type: "long".to_string(),
                        }),
                    },
                ],
                is_strict: false,
            },
            Pattern::Has {
                thing_var: format!("$e{}", i),
                attr_type: "label".to_string(),
                attr_var: format!("$lbl{}", i),
            },
            Pattern::ValueComparison {
                var: format!("$lbl{}", i),
                operator: "!=".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(""),
                    value_type: "string".to_string(),
                }),
            },
        ]));
    }
    for i in 0..60 {
        let var = format!("$ins{}", i);
        clauses.push(Clause::Insert(vec![
            Statement::Isa {
                variable: var.clone(),
                type_name: format!("record-{}", i % 15),
            },
            Statement::Has {
                subject_var: var.clone(),
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(format!("item-{}", i)),
                    value_type: "string".to_string(),
                }),
            },
            Statement::Has {
                subject_var: var.clone(),
                attr_name: "value".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(i as f64 * 3.15),
                    value_type: "double".to_string(),
                }),
            },
            Statement::Has {
                subject_var: var,
                attr_name: "timestamp".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!(format!("2025-06-{:02}", (i % 28) + 1)),
                    value_type: "date".to_string(),
                }),
            },
        ]));
    }
    for i in 0..40 {
        clauses.push(Clause::Delete(vec![Statement::DeleteThing(format!(
            "$del{}",
            i
        ))]));
    }
    for i in 0..20 {
        clauses.push(Clause::Update(vec![Statement::Has {
            subject_var: format!("$upd{}", i),
            attr_name: "modified".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!("2025-12-31"),
                value_type: "date".to_string(),
            }),
        }]));
    }
    clauses
}

// ---------------------------------------------------------------------------
// ArithmeticValue compilation
// ---------------------------------------------------------------------------

pub fn make_arithmetic_match() -> Clause {
    Clause::Match(vec![
        Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "employee".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "department".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("engineering"),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        },
        Pattern::Has {
            thing_var: "$p".to_string(),
            attr_type: "salary".to_string(),
            attr_var: "$salary".to_string(),
        },
        Pattern::Has {
            thing_var: "$p".to_string(),
            attr_type: "base-pay".to_string(),
            attr_var: "$base".to_string(),
        },
        Pattern::Has {
            thing_var: "$p".to_string(),
            attr_type: "bonus".to_string(),
            attr_var: "$bonus".to_string(),
        },
        Pattern::ValueComparison {
            var: "$salary".to_string(),
            operator: ">".to_string(),
            value: Value::Arithmetic(ArithmeticValue {
                left: Box::new(Value::Arithmetic(ArithmeticValue {
                    left: Box::new(Value::Variable("$base".to_string())),
                    operator: "+".to_string(),
                    right: Box::new(Value::Variable("$bonus".to_string())),
                })),
                operator: "*".to_string(),
                right: Box::new(Value::Literal(LiteralValue {
                    value: json!(1.5),
                    value_type: "double".to_string(),
                })),
            }),
        },
    ])
}

pub fn make_nested_arithmetic() -> Clause {
    Clause::Match(vec![
        Pattern::Entity {
            variable: "$x".to_string(),
            type_name: "calculation".to_string(),
            constraints: vec![],
            is_strict: false,
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "a".to_string(),
            attr_var: "$a".to_string(),
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "b".to_string(),
            attr_var: "$b".to_string(),
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "c".to_string(),
            attr_var: "$c".to_string(),
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "d".to_string(),
            attr_var: "$d".to_string(),
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "e".to_string(),
            attr_var: "$e".to_string(),
        },
        Pattern::Has {
            thing_var: "$x".to_string(),
            attr_type: "f".to_string(),
            attr_var: "$f".to_string(),
        },
        Pattern::ValueComparison {
            var: "$result".to_string(),
            operator: ">=".to_string(),
            value: Value::Arithmetic(ArithmeticValue {
                left: Box::new(Value::Arithmetic(ArithmeticValue {
                    left: Box::new(Value::Arithmetic(ArithmeticValue {
                        left: Box::new(Value::Variable("$a".to_string())),
                        operator: "+".to_string(),
                        right: Box::new(Value::Variable("$b".to_string())),
                    })),
                    operator: "*".to_string(),
                    right: Box::new(Value::Arithmetic(ArithmeticValue {
                        left: Box::new(Value::Variable("$c".to_string())),
                        operator: "-".to_string(),
                        right: Box::new(Value::Variable("$d".to_string())),
                    })),
                })),
                operator: "/".to_string(),
                right: Box::new(Value::Arithmetic(ArithmeticValue {
                    left: Box::new(Value::Arithmetic(ArithmeticValue {
                        left: Box::new(Value::Variable("$e".to_string())),
                        operator: "%".to_string(),
                        right: Box::new(Value::Variable("$f".to_string())),
                    })),
                    operator: "^".to_string(),
                    right: Box::new(Value::Literal(LiteralValue {
                        value: json!(2),
                        value_type: "long".to_string(),
                    })),
                })),
            }),
        },
    ])
}

pub fn make_reduce_with_groupby() -> Clause {
    Clause::Reduce {
        assignments: vec![
            ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".to_string(),
                    args: vec![Value::Variable("$p".to_string())],
                }),
            },
            ReduceAssignment {
                variable: "$total_salary".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "sum".to_string(),
                    args: vec![Value::Variable("$salary".to_string())],
                }),
            },
            ReduceAssignment {
                variable: "$max_age".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "max".to_string(),
                    args: vec![Value::Variable("$age".to_string())],
                }),
            },
            ReduceAssignment {
                variable: "$min_age".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "min".to_string(),
                    args: vec![Value::Variable("$age".to_string())],
                }),
            },
            ReduceAssignment {
                variable: "$avg_score".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "mean".to_string(),
                    args: vec![Value::Variable("$score".to_string())],
                }),
            },
        ],
        group_by: Some("$dept".to_string()),
    }
}

pub fn make_match_let() -> Clause {
    Clause::MatchLet(vec![
        LetAssignment {
            variables: vec!["$count".to_string()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "count".to_string(),
                args: vec![Value::Variable("$p".to_string())],
            }),
            is_stream: false,
        },
        LetAssignment {
            variables: vec!["$total".to_string()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "sum".to_string(),
                args: vec![Value::Variable("$salary".to_string())],
            }),
            is_stream: false,
        },
        LetAssignment {
            variables: vec!["$x".to_string()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "values".to_string(),
                args: vec![Value::Variable("$attr".to_string())],
            }),
            is_stream: true,
        },
    ])
}

pub fn make_fetch_variable() -> Clause {
    Clause::Fetch(vec![
        FetchItem::Variable {
            key: "person".to_string(),
            var: "$p".to_string(),
        },
        FetchItem::Variable {
            key: "company".to_string(),
            var: "$c".to_string(),
        },
        FetchItem::Attribute {
            key: "name".to_string(),
            var: "$p".to_string(),
            attr_name: "name".to_string(),
        },
        FetchItem::Function {
            key: "_iid".to_string(),
            func_name: "iid".to_string(),
            var: "$p".to_string(),
        },
    ])
}

pub fn make_isa_constraint() -> Clause {
    Clause::Match(vec![
        Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![
                Constraint::Isa {
                    type_name: "employee".to_string(),
                    strict: false,
                },
                Constraint::Has {
                    attr_name: "name".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!("Alice"),
                        value_type: "string".to_string(),
                    }),
                },
            ],
            is_strict: false,
        },
        Pattern::Entity {
            variable: "$a".to_string(),
            type_name: "animal".to_string(),
            constraints: vec![Constraint::Isa {
                type_name: "mammal".to_string(),
                strict: true,
            }],
            is_strict: false,
        },
        Pattern::Entity {
            variable: "$v".to_string(),
            type_name: "vehicle".to_string(),
            constraints: vec![
                Constraint::Isa {
                    type_name: "electric-vehicle".to_string(),
                    strict: false,
                },
                Constraint::Has {
                    attr_name: "range".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!(300),
                        value_type: "long".to_string(),
                    }),
                },
            ],
            is_strict: false,
        },
    ])
}

pub fn make_standalone_insert() -> Clause {
    let mut statements = Vec::new();
    for i in 0..5 {
        let var = format!("$e{}", i);
        statements.push(Statement::Isa {
            variable: var.clone(),
            type_name: format!("person-{}", i),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "name".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(format!("Person {}", i)),
                value_type: "string".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(),
            attr_name: "age".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(20 + i),
                value_type: "long".to_string(),
            }),
        });
        statements.push(Statement::Has {
            subject_var: var,
            attr_name: "active".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!(true),
                value_type: "boolean".to_string(),
            }),
        });
    }
    Clause::Insert(statements)
}

pub fn make_standalone_delete() -> Clause {
    let statements: Vec<Statement> = (0..10)
        .map(|i| Statement::DeleteThing(format!("$d{}", i)))
        .collect();
    Clause::Delete(statements)
}

pub fn make_standalone_update() -> Clause {
    let mut statements = Vec::new();
    for i in 0..10 {
        statements.push(Statement::Has {
            subject_var: format!("$u{}", i),
            attr_name: "modified-at".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!("2025-12-31"),
                value_type: "date".to_string(),
            }),
        });
    }
    for i in 0..10 {
        statements.push(Statement::Has {
            subject_var: format!("$u{}", i),
            attr_name: "status".to_string(),
            value: Value::Literal(LiteralValue {
                value: json!("updated"),
                value_type: "string".to_string(),
            }),
        });
    }
    Clause::Update(statements)
}
