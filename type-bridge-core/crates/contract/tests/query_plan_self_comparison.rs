#[path = "support/plan.rs"]
#[allow(dead_code)]
mod support;

use support::*;
use type_bridge_contract::id::AttributeId;
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::{
    DeleteItem, Expr, Operation, Pattern, PlanBuilder, QueryOperand, QueryOutput, QueryPattern,
    QueryPlan, Set, Stage, WriteReturns,
};
use type_bridge_contract::value::ValueTypeTag;

const CODE: &str = "query_plan_attribute_self_comparison";
const COMPARATORS: [ValueComparator; 6] = [
    ValueComparator::Equal,
    ValueComparator::NotEqual,
    ValueComparator::Less,
    ValueComparator::LessOrEqual,
    ValueComparator::Greater,
    ValueComparator::GreaterOrEqual,
];
fn attribute() -> QueryPattern {
    QueryPattern::Has {
        owner: id(0),
        attribute: id(1),
        attribute_id: AttributeId::new("balance").unwrap(),
    }
}
fn comparison(comparator: ValueComparator) -> QueryPattern {
    QueryPattern::Value {
        comparator,
        left: QueryOperand::Binding { binding: id(1) },
        right: QueryOperand::Binding { binding: id(1) },
    }
}
fn assert_refused(
    wrap: impl Fn(Vec<QueryPattern>) -> Vec<QueryPattern>,
    operation: Operation,
    tail: Vec<Stage>,
) {
    for comparator in COMPARATORS {
        let mut patterns = vec![isa(0, entity("account")).into()];
        patterns.extend(
            wrap(vec![attribute(), comparison(comparator)])
                .into_iter()
                .map(Pattern::Read),
        );
        let mut stages = vec![Stage::Match { patterns }];
        stages.extend(tail.clone());
        let mut request = spec(stages, vec![0]);
        request.operation = operation;
        if operation != Operation::Rows {
            request.output = QueryOutput::Rows { columns: vec![] };
        }
        let error = QueryPlan::new(request, &schema()).unwrap_err();
        assert_eq!(error.code().as_str(), CODE, "{comparator:?}");
        assert!(error.message().contains("TypeDB 3.12-3.14"));
        assert!(error.message().contains("independent witnesses"));
    }
}
#[test]
fn read_refuses_attribute_self_comparison() {
    assert_refused(|patterns| patterns, Operation::Rows, vec![]);
}
#[test]
fn count_refuses_attribute_self_comparison() {
    assert_refused(|patterns| patterns, Operation::Count, vec![]);
}
#[test]
fn exists_refuses_attribute_self_comparison() {
    assert_refused(|patterns| patterns, Operation::Exists, vec![]);
}
#[test]
fn delete_filter_refuses_attribute_self_comparison() {
    assert_refused(
        |patterns| patterns,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![Stage::Delete {
            items: vec![DeleteItem::Thing { binding: id(0) }],
        }],
    );
}
#[test]
fn update_filter_refuses_attribute_self_comparison() {
    assert_refused(
        |patterns| patterns,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![Stage::Update {
            sets: vec![Set::Field {
                owner: id(0),
                field: field("balance"),
                operand: literal(1),
            }],
        }],
    );
}
#[test]
fn nested_disjunction_refuses_attribute_self_comparison() {
    assert_refused(
        |patterns| {
            vec![QueryPattern::Or {
                branches: vec![patterns.clone(), patterns],
            }]
        },
        Operation::Count,
        vec![],
    );
}
#[test]
fn negated_scope_refuses_attribute_self_comparison() {
    assert_refused(
        |patterns| vec![QueryPattern::Not { patterns }],
        Operation::Count,
        vec![],
    );
}
#[test]
fn optional_scope_refuses_attribute_self_comparison() {
    assert_refused(
        |patterns| vec![QueryPattern::Try { patterns }],
        Operation::Count,
        vec![],
    );
}
#[test]
fn builder_refuses_attribute_self_comparison() {
    for comparator in COMPARATORS {
        let mut plan = PlanBuilder::new();
        let owner = plan.exact(entity("account"));
        let value = plan.field(owner, AttributeId::new("balance").unwrap());
        plan.compare(
            QueryOperand::Binding { binding: value },
            comparator,
            QueryOperand::Binding { binding: value },
        );
        plan.count();
        assert_eq!(
            plan.build(&schema(), semantics())
                .unwrap_err()
                .code()
                .as_str(),
            CODE
        );
    }
}
#[test]
fn scalar_literal_and_input_self_comparisons_remain_valid() {
    for comparator in COMPARATORS {
        let mut literal_plan = PlanBuilder::new();
        literal_plan.exact(entity("account"));
        literal_plan.compare(literal(1), comparator, literal(1));
        literal_plan.count();
        literal_plan.build(&schema(), semantics()).unwrap();
        let mut input_plan = PlanBuilder::new();
        input_plan.exact(entity("account"));
        let column = input_plan.input("value", ValueTypeTag::Long, false);
        input_plan.compare(
            QueryOperand::Input { column },
            comparator,
            QueryOperand::Input { column },
        );
        input_plan.count();
        input_plan.build(&schema(), semantics()).unwrap();
        let mut request = spec(
            vec![Stage::Match {
                patterns: vec![
                    isa(0, entity("account")).into(),
                    Pattern::Let {
                        binding: id(1),
                        expr: Expr::Operand(literal(1)),
                    },
                    comparison(comparator).into(),
                ],
            }],
            vec![],
        );
        request.operation = Operation::Count;
        QueryPlan::new(request, &schema()).unwrap();
    }
}
#[test]
fn independent_attribute_witnesses_remain_valid() {
    let mut request = spec(
        vec![Stage::Match {
            patterns: vec![
                isa(0, entity("account")).into(),
                attribute().into(),
                QueryPattern::Has {
                    owner: id(0),
                    attribute: id(2),
                    attribute_id: AttributeId::new("balance").unwrap(),
                }
                .into(),
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: id(1) },
                    right: QueryOperand::Binding { binding: id(2) },
                }
                .into(),
            ],
        }],
        vec![],
    );
    request.operation = Operation::Count;
    QueryPlan::new(request, &schema()).unwrap();
}
