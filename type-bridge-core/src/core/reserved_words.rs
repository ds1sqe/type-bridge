use std::collections::HashSet;
use once_cell::sync::Lazy;

pub static TYPEQL_RESERVED_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut m = HashSet::new();
    // Schema queries
    m.insert("define");
    m.insert("undefine");
    m.insert("redefine");
    // Data manipulation stages
    m.insert("match");
    m.insert("fetch");
    m.insert("insert");
    m.insert("delete");
    m.insert("update");
    m.insert("put");
    // Stream manipulation stages
    m.insert("select");
    m.insert("require");
    m.insert("sort");
    m.insert("limit");
    m.insert("offset");
    m.insert("reduce");
    // Special stages
    m.insert("with");
    // Pattern logic
    m.insert("or");
    m.insert("not");
    m.insert("try");
    // Type definition statements
    m.insert("entity");
    m.insert("relation");
    m.insert("attribute");
    m.insert("struct");
    m.insert("fun");
    // Constraint definition statements
    m.insert("sub");
    m.insert("relates");
    m.insert("plays");
    m.insert("value");
    m.insert("owns");
    m.insert("alias");
    // Instance statements
    m.insert("isa");
    m.insert("links");
    m.insert("has");
    m.insert("is");
    m.insert("let");
    m.insert("contains");
    m.insert("like");
    // Identity statements
    m.insert("label");
    m.insert("iid");
    // Annotations
    m.insert("card");
    m.insert("cascade");
    m.insert("independent");
    m.insert("abstract");
    m.insert("key");
    m.insert("subkey");
    m.insert("unique");
    m.insert("values");
    m.insert("range");
    m.insert("regex");
    m.insert("distinct");
    // Reductions
    m.insert("check");
    m.insert("first");
    m.insert("count");
    m.insert("max");
    m.insert("min");
    m.insert("mean");
    m.insert("median");
    m.insert("std");
    m.insert("sum");
    m.insert("list");
    // Value types
    m.insert("boolean");
    m.insert("integer");
    m.insert("double");
    m.insert("decimal");
    m.insert("datetime-tz");
    m.insert("datetime_tz");
    m.insert("datetime");
    m.insert("date");
    m.insert("duration");
    m.insert("string");
    // Built-in functions
    m.insert("round");
    m.insert("ceil");
    m.insert("floor");
    m.insert("abs");
    m.insert("length");
    // Literals
    m.insert("true");
    m.insert("false");
    // Miscellaneous
    m.insert("asc");
    m.insert("desc");
    m.insert("return");
    m.insert("of");
    m.insert("from");
    m.insert("in");
    m.insert("as");
    m
});

pub fn is_reserved_word(name: &str) -> bool {
    TYPEQL_RESERVED_WORDS.contains(name.to_lowercase().as_str())
}
