use std::collections::BTreeSet;

use serde_json::{Value, json};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::limits::CANONICAL_CODEC_LIMITS;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocText, DocumentId, FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
    FunctionReturnMode, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
    RelatesFact, RelatesFactId, SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact,
    StructFact, StructField, SubFact, SubFactId, TypeFact, TypeReference, ValueFact, ValueFactId,
    decode_declared_schema, encode_declared_schema,
};
use type_bridge_contract::value::ValueTypeTag;

const COMPILED_PROVENANCE_DOCUMENT: &str = "__typebridge_compiled__/declared-schema-v1";

fn capability(value: &str) -> CapabilityId {
    CapabilityId::new(value).unwrap()
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn declared_schema() -> DeclaredSchema {
    let person = type_id(TypeKind::Entity, "person");
    let employee = type_id(TypeKind::Entity, "employee");
    let name_type = type_id(TypeKind::Attribute, "name");
    let name = AttributeId::new("name").unwrap();
    let friendship = type_id(TypeKind::Relation, "friendship");
    let friend = RoleId::new("friendship", "friend").unwrap();
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(employee.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(name_type).unwrap()),
        SchemaFact::Type(TypeFact::new(friendship.clone()).unwrap()),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person.clone()).unwrap(),
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name).unwrap(),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(friendship, friend.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), friend).unwrap(),
        )),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Type(person.clone()),
                    AnnotationKindId::Doc,
                ),
                SchemaAnnotationValue::Doc(DocText::new("Person documentation").unwrap()),
            )
            .unwrap(),
        ),
        SchemaFact::Function(FunctionFact::new(
            FunctionId::new("find_person").unwrap(),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("input").unwrap(),
                    TypeReference::Schema(Label::new("person").unwrap()),
                )],
                FunctionReturnMode::stream(vec![FunctionReturnElement::new(
                    TypeReference::Schema(Label::new("person").unwrap()),
                    false,
                )])
                .unwrap(),
            )
            .unwrap(),
            FunctionBody::new("match $input isa person; return { $input }; ").unwrap(),
        )),
        SchemaFact::Struct(
            StructFact::new(
                StructId::new("name_record").unwrap(),
                vec![
                    StructField::new(Label::new("value").unwrap(), ValueTypeTag::String, false),
                    StructField::new(Label::new("alias").unwrap(), ValueTypeTag::String, true),
                ],
            )
            .unwrap(),
        ),
    ];
    let document = DocumentId::new("authoring/schema.yaml").unwrap();
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let column = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(document.clone(), byte, byte + 1, 1, column, 1, column + 1).unwrap(),
        )
    });
    DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::from_iter([capability("schema.functions"), capability("schema.structs")]),
        sourced,
    )
    .unwrap()
}

fn canonical_value(schema: &DeclaredSchema) -> Value {
    serde_json::from_slice(&encode_declared_schema(schema).unwrap()).unwrap()
}

fn rehash_declared_identity(value: &mut Value) {
    let identity = json!({
        "facts": value["facts"].clone(),
        "format_version": value["format_version"].clone(),
        "required_capabilities": value["required_capabilities"].clone(),
    });
    let bytes = to_canonical_json(&identity).unwrap();
    let fingerprint = Fingerprint::compute(
        FingerprintDomain::new("typebridge.schema.declared-identity").unwrap(),
        CanonicalizationVersion::new("typebridge.schema-canonical-json/v1").unwrap(),
        None,
        &bytes,
    );
    value["declared_identity"] = serde_json::to_value(fingerprint).unwrap();
}

#[test]
fn all_fact_variants_round_trip_with_stable_bytes_and_compiled_provenance() {
    let schema = declared_schema();
    let first = encode_declared_schema(&schema).unwrap();
    assert_eq!(encode_declared_schema(&schema).unwrap(), first);
    assert_eq!(
        serde_json::to_value(&schema).unwrap(),
        canonical_value(&schema)
    );

    let value: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(
        value["declared_identity"],
        serde_json::to_value(schema.declared_identity_fingerprint()).unwrap(),
    );
    let kinds = value["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fact| fact["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "annotation",
            "function",
            "owns",
            "plays",
            "relates",
            "struct",
            "sub",
            "type",
            "value",
        ]),
    );

    let decoded = decode_declared_schema(&first).unwrap();
    assert_eq!(decoded.format(), schema.format());
    assert_eq!(
        decoded.required_capabilities(),
        schema.required_capabilities(),
    );
    assert_eq!(
        decoded.facts().collect::<Vec<_>>(),
        schema.facts().collect::<Vec<_>>(),
    );
    assert_eq!(
        decoded.canonical_identity_bytes().unwrap(),
        schema.canonical_identity_bytes().unwrap(),
    );
    assert_eq!(
        decoded.declared_identity_fingerprint(),
        schema.declared_identity_fingerprint(),
    );
    assert_eq!(encode_declared_schema(&decoded).unwrap(), first);
    for fact in decoded.facts() {
        let source = serde_json::to_value(decoded.source(&fact.id()).unwrap()).unwrap();
        assert_eq!(source["document"], COMPILED_PROVENANCE_DOCUMENT);
    }
}

#[test]
fn invalid_but_rehashed_facts_still_reenter_validating_constructors() {
    let schema = declared_schema();

    let mut missing_reference = canonical_value(&schema);
    let facts = missing_reference["facts"].as_array_mut().unwrap();
    let index = facts
        .iter()
        .position(|fact| fact["kind"] == "type" && fact["value"]["id"]["label"] == "person")
        .unwrap();
    facts.remove(index);
    rehash_declared_identity(&mut missing_reference);
    assert!(decode_declared_schema(&to_canonical_json(&missing_reference).unwrap()).is_err());

    let mut duplicate = canonical_value(&schema);
    let duplicate_fact = duplicate["facts"][0].clone();
    duplicate["facts"]
        .as_array_mut()
        .unwrap()
        .push(duplicate_fact);
    rehash_declared_identity(&mut duplicate);
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&duplicate).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "duplicate_schema_fact",
    );

    let mut invalid_struct = canonical_value(&schema);
    let structure = invalid_struct["facts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|fact| fact["kind"] == "struct")
        .unwrap();
    let field = structure["value"]["fields"][0].clone();
    structure["value"]["fields"]
        .as_array_mut()
        .unwrap()
        .push(field);
    rehash_declared_identity(&mut invalid_struct);
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&invalid_struct).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "duplicate_struct_field",
    );
}

#[test]
fn decoder_rejects_unknown_noncanonical_malformed_oversize_and_deep_inputs() {
    let schema = declared_schema();

    let mut unknown_root = canonical_value(&schema);
    unknown_root
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), Value::Bool(true));
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&unknown_root).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut unknown_fact = canonical_value(&schema);
    unknown_fact["facts"][0]["value"]
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), Value::Bool(true));
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&unknown_fact).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let bytes = encode_declared_schema(&schema).unwrap();
    let mut spaced = Vec::with_capacity(bytes.len() + 1);
    spaced.push(b' ');
    spaced.extend_from_slice(&bytes);
    assert_eq!(
        decode_declared_schema(&spaced).unwrap_err().code().as_str(),
        "non_canonical_json",
    );
    assert_eq!(
        decode_declared_schema(b"{").unwrap_err().code().as_str(),
        "malformed_canonical_json",
    );

    let oversized = vec![b' '; CANONICAL_CODEC_LIMITS.max_bytes + 1];
    assert_eq!(
        decode_declared_schema(&oversized)
            .unwrap_err()
            .code()
            .as_str(),
        "canonical_json_too_large",
    );

    let depth = CANONICAL_CODEC_LIMITS.max_depth + 1;
    let mut too_deep = "[".repeat(depth);
    too_deep.push('0');
    too_deep.push_str(&"]".repeat(depth));
    assert_eq!(
        decode_declared_schema(too_deep.as_bytes())
            .unwrap_err()
            .code()
            .as_str(),
        "canonical_json_too_deep",
    );
}

#[test]
fn decoder_rejects_order_payload_capability_and_fingerprint_tampering() {
    let schema = declared_schema();

    let mut reordered = canonical_value(&schema);
    reordered["facts"].as_array_mut().unwrap().reverse();
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&reordered).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "non_canonical_declared_schema",
    );

    let mut payload = canonical_value(&schema);
    let function = payload["facts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|fact| fact["kind"] == "function")
        .unwrap();
    function["value"]["body"] = Value::String("return changed;".to_owned());
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&payload).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "declared_schema_fingerprint_mismatch",
    );

    let mut capabilities = canonical_value(&schema);
    capabilities["required_capabilities"][0] = Value::String("schema.changed".to_owned());
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&capabilities).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "declared_schema_fingerprint_mismatch",
    );

    let mut fingerprint = canonical_value(&schema);
    fingerprint["declared_identity"]["digest"] = Value::String("0".repeat(64));
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&fingerprint).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "declared_schema_fingerprint_mismatch",
    );

    let mut domain = canonical_value(&schema);
    domain["declared_identity"]["domain"] =
        Value::String("typebridge.schema.wrong-domain".to_owned());
    assert_eq!(
        decode_declared_schema(&to_canonical_json(&domain).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_declared_identity_fingerprint",
    );
}
