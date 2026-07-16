use type_bridge_contract::id::{FunctionId, Label};
use type_bridge_contract::schema::{
    FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
    FunctionSignature, TypeReference,
};
use type_bridge_contract::value::ValueTypeTag;

fn value(tag: ValueTypeTag) -> TypeReference {
    TypeReference::Value(tag)
}

#[test]
fn function_signature_preserves_order_and_body_bytes() {
    let signature = FunctionSignature::new(
        vec![
            FunctionParameter::new(Label::new("left").unwrap(), value(ValueTypeTag::Long)),
            FunctionParameter::new(Label::new("right").unwrap(), value(ValueTypeTag::Long)),
        ],
        FunctionReturnMode::stream(vec![FunctionReturnElement::new(
            value(ValueTypeTag::Boolean),
            false,
        )])
        .unwrap(),
    )
    .unwrap();
    let body = FunctionBody::new("match\n  # keep me\nreturn { true };\n").unwrap();
    let fact = FunctionFact::new(FunctionId::new("compare").unwrap(), signature, body);

    assert_eq!(fact.signature().parameters()[0].name().as_str(), "left");
    assert_eq!(fact.signature().parameters()[1].name().as_str(), "right");
    assert_eq!(fact.body().text(), "match\n  # keep me\nreturn { true };\n");
}

#[test]
fn function_contract_rejects_invalid_shapes() {
    let parameter = FunctionParameter::new(
        Label::new("value").unwrap(),
        value(ValueTypeTag::String),
    );
    let returns = FunctionReturnMode::scalar(FunctionReturnElement::new(
        value(ValueTypeTag::String),
        false,
    ));
    assert_eq!(
        FunctionSignature::new(vec![parameter.clone(), parameter], returns)
            .unwrap_err()
            .code()
            .as_str(),
        "duplicate_function_parameter",
    );
    assert_eq!(
        FunctionReturnMode::stream(Vec::new())
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_function_stream_return",
    );
    assert_eq!(
        FunctionBody::new("").unwrap_err().code().as_str(),
        "invalid_function_body",
    );
}

#[test]
fn type_position_tokens_are_unambiguous() {
    assert_eq!(
        TypeReference::from_token("integer").unwrap(),
        TypeReference::Value(ValueTypeTag::Long),
    );
    assert_eq!(
        TypeReference::from_token("person").unwrap(),
        TypeReference::Schema(Label::new("person").unwrap()),
    );
}
