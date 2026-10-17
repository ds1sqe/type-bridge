//! Semantic comparisons for newly emitted queries; carried-byte tests stay exact.
use serde_json::Value;

pub(super) fn assert_values(left: &Value, right: &Value, query: bool) {
    if left == right {
        return;
    }
    match (left, right) {
        (Value::String(left), Value::String(right)) if query => {
            assert!(
                type_bridge_core_lib::__internal::equivalent(left, right).unwrap_or_else(
                    |error| panic!("query comparison failed: {error}: {left} / {right}")
                ),
                "query meanings differ: {left} / {right}"
            );
        }
        (Value::Array(left), Value::Array(right)) => {
            assert_eq!(left.len(), right.len());
            for (left, right) in left.iter().zip(right) {
                assert_values(left, right, query);
            }
        }
        (Value::Object(left), Value::Object(right)) => {
            assert_eq!(
                left.keys().collect::<Vec<_>>(),
                right.keys().collect::<Vec<_>>()
            );
            for (key, left) in left {
                assert_values(
                    left,
                    &right[key],
                    matches!(key.as_str(), "forward" | "reverse"),
                );
            }
        }
        _ => assert_eq!(left, right),
    }
}

macro_rules! assert_typeql_eq {
    ($left:expr, $right:expr $(,)?) => {
        $crate::typeql_test::assert_values(
            &serde_json::to_value(&$left).unwrap(),
            &serde_json::to_value(&$right).unwrap(),
            true,
        )
    };
}
