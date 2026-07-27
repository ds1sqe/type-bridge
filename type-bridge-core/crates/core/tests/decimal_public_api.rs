use std::cmp::Ordering;

use type_bridge_core_lib::decimal::{CanonicalDecimal, parse_decimal};

#[test]
fn released_decimal_module_path_remains_callable() {
    let legacy: CanonicalDecimal<'_> =
        parse_decimal("001.2300dec").expect("legacy decimal path must parse");
    let canonical = parse_decimal("1.23").expect("comparison value must parse");

    assert_eq!(legacy.compare(&canonical), Ordering::Equal);
    assert_eq!(parse_decimal("-0.000dec"), parse_decimal("0"));
}
