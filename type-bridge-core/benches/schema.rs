use criterion::{black_box, criterion_group, criterion_main, Criterion};
use type_bridge_core::core::schema::TypeSchema;

const SMALL_SCHEMA: &str = r#"
define
attribute name, value string;
attribute email, value string;
attribute age, value long;
entity person, owns name @key, owns email @unique, owns age;
relation friendship, relates friend;
"#;

const MEDIUM_SCHEMA: &str = r#"
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

fn bench_parse_small_schema(c: &mut Criterion) {
    c.bench_function("schema/parse_small", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(SMALL_SCHEMA)).unwrap())
    });
}

fn bench_parse_medium_schema(c: &mut Criterion) {
    c.bench_function("schema/parse_medium", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(MEDIUM_SCHEMA)).unwrap())
    });
}

fn bench_parse_large_schema(c: &mut Criterion) {
    // Generate a large schema with many types
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

    c.bench_function("schema/parse_large_120types", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(&tql)).unwrap())
    });
}

fn bench_resolve_inheritance(c: &mut Criterion) {
    // Schema with a deep inheritance chain
    let mut tql = String::from("define\n");
    tql.push_str("attribute name, value string;\n");
    tql.push_str("entity level-0 @abstract, owns name;\n");
    for i in 1..=10 {
        tql.push_str(&format!(
            "attribute attr-{}, value string;\n",
            i
        ));
        tql.push_str(&format!(
            "entity level-{} sub level-{}, owns attr-{};\n",
            i,
            i - 1,
            i
        ));
    }

    c.bench_function("schema/resolve_inheritance_depth10", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(&tql)).unwrap())
    });
}

fn bench_json_round_trip(c: &mut Criterion) {
    let schema = TypeSchema::from_typeql(MEDIUM_SCHEMA).unwrap();
    let json = schema.to_json().unwrap();

    c.bench_function("schema/to_json", |b| {
        b.iter(|| black_box(&schema).to_json().unwrap())
    });

    c.bench_function("schema/from_json", |b| {
        b.iter(|| TypeSchema::from_json(black_box(&json)).unwrap())
    });
}

criterion_group!(
    benches,
    bench_parse_small_schema,
    bench_parse_medium_schema,
    bench_parse_large_schema,
    bench_resolve_inheritance,
    bench_json_round_trip,
);
criterion_main!(benches);
