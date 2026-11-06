//! Bench inputs shared by the Criterion (wall time) and gungraun (instruction
//! count) benches, so both measure the same work under the same ids.
// Each bench binary compiles this module but uses only its own fixtures.
#![allow(dead_code)]

mod gungraun;
#[allow(unused_imports)] // Only the gungraun bench binaries use it.
pub(crate) use gungraun::gungraun_main;

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

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

pub const SMALL_SCHEMA: &str = r#"
define
attribute name, value string;
attribute email, value string;
attribute age, value integer;
entity person, owns name @key, owns email @unique, owns age;
relation friendship, relates friend;
"#;

pub const MEDIUM_SCHEMA: &str = r#"
define
attribute name, value string;
attribute email, value string;
attribute age, value integer;
attribute title, value string;
attribute isbn, value string;
attribute price, value double;
attribute rating, value double;
attribute published-date, value date;
attribute page-count, value integer;
attribute description, value string;

entity base-entity @abstract, owns name;

entity person, sub base-entity,
    owns email @key,
    owns age;

entity author, sub person;
entity editor, sub person;
entity reviewer, sub person;

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
            "entity level-{}, sub level-{}, owns attr-{};\n",
            i,
            i - 1,
            i
        ));
    }
    tql
}
