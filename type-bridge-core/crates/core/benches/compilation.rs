use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;
use type_bridge_core_lib::ast::{
    ArithmeticValue, Clause, Constraint, FetchItem, FunctionCallValue, LetAssignment, LiteralValue,
    Pattern, ReduceAssignment, RolePlayer, Statement, Value,
};
use type_bridge_core_lib::compiler::QueryCompiler;

fn make_simple_match() -> Clause {
    Clause::Match(vec![Pattern::Entity {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![],
        is_strict: false,
    }])
}

fn make_match_with_constraints() -> Clause {
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

fn make_complex_query() -> Vec<Clause> {
    let match_clause = Clause::Match(vec![
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
            constraints: vec![
                Constraint::Has {
                    attr_name: "budget".to_string(),
                    value: Value::Literal(LiteralValue {
                        value: json!(100000.0),
                        value_type: "double".to_string(),
                    }),
                },
            ],
            is_strict: false,
        },
        Pattern::Iid {
            variable: "$x".to_string(),
            iid: "0xabcdef".to_string(),
        },
        Pattern::Attribute {
            variable: "$a".to_string(),
            type_name: "salary".to_string(),
            value: Some(Value::Literal(LiteralValue {
                value: json!(75000),
                value_type: "long".to_string(),
            })),
        },
    ]);

    let fetch_clause = Clause::Fetch(vec![
        FetchItem::Attribute {
            key: "name".to_string(),
            var: "$p".to_string(),
            attr_name: "name".to_string(),
        },
        FetchItem::Attribute {
            key: "email".to_string(),
            var: "$p".to_string(),
            attr_name: "email".to_string(),
        },
        FetchItem::Wildcard {
            key: "company".to_string(),
            var: "$c".to_string(),
        },
    ]);

    vec![match_clause, fetch_clause]
}

fn make_batch_clauses() -> Vec<Clause> {
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

fn make_relation_insert() -> Clause {
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

fn bench_compile_simple_match(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_simple_match();

    c.bench_function("compile/simple_match", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_match_with_constraints(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_match_with_constraints();

    c.bench_function("compile/match_with_constraints", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_complex_query(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clauses = make_complex_query();

    c.bench_function("compile/complex_10_patterns", |b| {
        b.iter(|| compiler.compile(black_box(&clauses)))
    });
}

fn bench_compile_batch(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clauses = make_batch_clauses();

    c.bench_function("compile/batch_50_clauses", |b| {
        b.iter(|| compiler.compile(black_box(&clauses)))
    });
}

fn bench_compile_relation_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_relation_insert();

    c.bench_function("compile/relation_insert", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_reduce(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = Clause::Reduce {
        assignments: vec![
            ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".to_string(),
                    args: vec![Value::Variable("$p".to_string())],
                }),
            },
            ReduceAssignment {
                variable: "$total".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "sum".to_string(),
                    args: vec![Value::Variable("$salary".to_string())],
                }),
            },
        ],
        group_by: Some("$dept".to_string()),
    };

    c.bench_function("compile/reduce_with_groupby", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Long query: 30 patterns (wide join)
// ---------------------------------------------------------------------------

fn make_long_query() -> Clause {
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

fn bench_compile_long_query(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_long_query();

    c.bench_function("compile/long_30_patterns", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Deeply nested: or-of-not-of-or, 3 levels
// ---------------------------------------------------------------------------

fn make_deeply_nested() -> Clause {
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

fn bench_compile_deeply_nested(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_deeply_nested();

    c.bench_function("compile/deeply_nested_3_levels", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Graph traversal: 8-hop chain
// ---------------------------------------------------------------------------

fn make_graph_traversal() -> Clause {
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

fn bench_compile_graph_traversal(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_graph_traversal();

    c.bench_function("compile/graph_traversal_8_hops", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Heavy insert: 100 entities x 6 attributes = 600 statements
// ---------------------------------------------------------------------------

fn make_heavy_insert() -> Clause {
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

fn bench_compile_heavy_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_heavy_insert();

    c.bench_function("compile/heavy_insert_100x6", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Large batch: 200 mixed clauses
// ---------------------------------------------------------------------------

fn make_large_batch() -> Vec<Clause> {
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

fn bench_compile_large_batch(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clauses = make_large_batch();

    c.bench_function("compile/large_batch_200_clauses", |b| {
        b.iter(|| compiler.compile(black_box(&clauses)))
    });
}

// ---------------------------------------------------------------------------
// ArithmeticValue compilation
// ---------------------------------------------------------------------------

fn make_arithmetic_match() -> Clause {
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

fn make_nested_arithmetic() -> Clause {
    Clause::Match(vec![
        Pattern::Entity {
            variable: "$x".to_string(),
            type_name: "calculation".to_string(),
            constraints: vec![],
            is_strict: false,
        },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "a".to_string(), attr_var: "$a".to_string() },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "b".to_string(), attr_var: "$b".to_string() },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "c".to_string(), attr_var: "$c".to_string() },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "d".to_string(), attr_var: "$d".to_string() },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "e".to_string(), attr_var: "$e".to_string() },
        Pattern::Has { thing_var: "$x".to_string(), attr_type: "f".to_string(), attr_var: "$f".to_string() },
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

fn bench_compile_arithmetic(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_arithmetic_match();
    c.bench_function("compile/arithmetic_match", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_nested_arithmetic(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = make_nested_arithmetic();
    c.bench_function("compile/nested_arithmetic_4_levels", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// MatchLetClause compilation
// ---------------------------------------------------------------------------

fn bench_compile_match_let(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = Clause::MatchLet(vec![
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
    ]);
    c.bench_function("compile/match_let_3_assignments", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// FetchVariable compilation
// ---------------------------------------------------------------------------

fn bench_compile_fetch_variable(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = Clause::Fetch(vec![
        FetchItem::Variable { key: "person".to_string(), var: "$p".to_string() },
        FetchItem::Variable { key: "company".to_string(), var: "$c".to_string() },
        FetchItem::Attribute { key: "name".to_string(), var: "$p".to_string(), attr_name: "name".to_string() },
        FetchItem::Function { key: "_iid".to_string(), func_name: "iid".to_string(), var: "$p".to_string() },
    ]);
    c.bench_function("compile/fetch_variable", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// IsaConstraint compilation
// ---------------------------------------------------------------------------

fn bench_compile_isa_constraint(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = Clause::Match(vec![
        Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![
                Constraint::Isa { type_name: "employee".to_string(), strict: false },
                Constraint::Has {
                    attr_name: "name".to_string(),
                    value: Value::Literal(LiteralValue { value: json!("Alice"), value_type: "string".to_string() }),
                },
            ],
            is_strict: false,
        },
        Pattern::Entity {
            variable: "$a".to_string(),
            type_name: "animal".to_string(),
            constraints: vec![
                Constraint::Isa { type_name: "mammal".to_string(), strict: true },
            ],
            is_strict: false,
        },
    ]);
    c.bench_function("compile/isa_constraint", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Standalone clause types
// ---------------------------------------------------------------------------

fn bench_compile_standalone_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let mut statements = Vec::new();
    for i in 0..5 {
        let var = format!("$e{}", i);
        statements.push(Statement::Isa { variable: var.clone(), type_name: format!("person-{}", i) });
        statements.push(Statement::Has {
            subject_var: var.clone(), attr_name: "name".to_string(),
            value: Value::Literal(LiteralValue { value: json!(format!("Person {}", i)), value_type: "string".to_string() }),
        });
        statements.push(Statement::Has {
            subject_var: var.clone(), attr_name: "age".to_string(),
            value: Value::Literal(LiteralValue { value: json!(20 + i), value_type: "long".to_string() }),
        });
        statements.push(Statement::Has {
            subject_var: var, attr_name: "active".to_string(),
            value: Value::Literal(LiteralValue { value: json!(true), value_type: "boolean".to_string() }),
        });
    }
    let clause = Clause::Insert(statements);
    c.bench_function("compile/standalone_insert_5x4", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_standalone_delete(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let statements: Vec<Statement> = (0..10)
        .map(|i| Statement::DeleteThing(format!("$d{}", i)))
        .collect();
    let clause = Clause::Delete(statements);
    c.bench_function("compile/standalone_delete_10", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_standalone_update(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let mut statements = Vec::new();
    for i in 0..10 {
        statements.push(Statement::Has {
            subject_var: format!("$u{}", i), attr_name: "modified-at".to_string(),
            value: Value::Literal(LiteralValue { value: json!("2025-12-31"), value_type: "date".to_string() }),
        });
    }
    for i in 0..10 {
        statements.push(Statement::Has {
            subject_var: format!("$u{}", i), attr_name: "status".to_string(),
            value: Value::Literal(LiteralValue { value: json!("updated"), value_type: "string".to_string() }),
        });
    }
    let clause = Clause::Update(statements);
    c.bench_function("compile/standalone_update_20", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

criterion_group!(
    benches,
    bench_compile_simple_match,
    bench_compile_match_with_constraints,
    bench_compile_complex_query,
    bench_compile_batch,
    bench_compile_relation_insert,
    bench_compile_reduce,
    bench_compile_long_query,
    bench_compile_deeply_nested,
    bench_compile_graph_traversal,
    bench_compile_heavy_insert,
    bench_compile_large_batch,
    bench_compile_arithmetic,
    bench_compile_nested_arithmetic,
    bench_compile_match_let,
    bench_compile_fetch_variable,
    bench_compile_isa_constraint,
    bench_compile_standalone_insert,
    bench_compile_standalone_delete,
    bench_compile_standalone_update,
);
criterion_main!(benches);
