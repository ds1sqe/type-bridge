//! Fixed Rust authority for the public Python/Node authoring inventory corpus.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{AttributeId, FunctionId, Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::query_plan::{
    DocumentSource, QueryOperand, QueryOperation, QueryOutput, QueryPattern, QueryPlan, ReadStage,
    decode_query_invocation, decode_query_plan, query_given_rows_capability,
    query_plan_authoring_capability_vocabulary,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, FunctionBody, FunctionFact, FunctionParameter,
    FunctionReturnElement, FunctionReturnMode, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact,
    PlaysFactId, RelatesFact, RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
    TypeReference, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_orm::query_v2_builder::{
    AuthoredQueryInvocation, AuthoredQueryPlan, QUERY_BUILDER_COMPARATORS,
    QUERY_BUILDER_LOCAL_RETURNS, QUERY_BUILDER_ORDER_DIRECTIONS, QUERY_BUILDER_REDUCERS,
    QUERY_BUILDER_TYPE_KINDS, QUERY_BUILDER_VALUE_TYPES, QUERY_PLAN_BUILDER_OPERATIONS,
    QueryBindingHandle, QueryDocumentFieldHandle, QueryInputHandle, QueryLocalFunctionHandle,
    QueryLocalReturnHandle, QueryOperandHandle, QueryOrderHandle, QueryPatternHandle,
    QueryPlanBuilder, QueryReduceAssignmentHandle, query_builder_comparator,
    query_builder_comparator_name, query_builder_function_target, query_builder_local_parameters,
    query_builder_order_direction, query_builder_order_direction_name, query_builder_reducer,
    query_builder_reducer_name, query_builder_role_players, query_builder_type_kind,
    query_builder_type_kind_name, query_builder_value_type, query_builder_value_type_name,
};
use type_bridge_orm::query_v2_prepared::QueryAuthority;

const FACADE_TERMINALS: &[&str] = &["documents"];
const ISA_MODES: &[&str] = &["exact", "include_subtypes"];
const FUNCTION_TARGET_KINDS: &[&str] = &["schema", "local"];
const TRANSPORT_VARIANTS: &[&str] = &["datetime_tz_single", "multirow", "optional_null"];

const fn operation_name(operation: QueryOperation) -> &'static str {
    match operation {
        QueryOperation::Rows => "rows",
        QueryOperation::Count => "count",
        QueryOperation::Exists => "exists",
    }
}

#[derive(Default)]
struct ObservedPlanInventory {
    type_kinds: BTreeSet<&'static str>,
    value_types: BTreeSet<&'static str>,
    comparators: BTreeSet<&'static str>,
    reducers: BTreeSet<&'static str>,
    operand_kinds: BTreeSet<&'static str>,
    pattern_kinds: BTreeSet<&'static str>,
    stage_kinds: BTreeSet<&'static str>,
    output_kinds: BTreeSet<&'static str>,
    document_source_kinds: BTreeSet<&'static str>,
    order_directions: BTreeSet<&'static str>,
    isa_modes: BTreeSet<&'static str>,
    function_target_kinds: BTreeSet<&'static str>,
    local_return_kinds: BTreeSet<String>,
}

fn observe_operand(operand: &QueryOperand, observed: &mut ObservedPlanInventory) {
    match operand {
        QueryOperand::Binding { .. } => {
            observed.operand_kinds.insert("binding");
        }
        QueryOperand::Literal { value } => {
            observed.operand_kinds.insert("literal");
            observed
                .value_types
                .insert(query_builder_value_type_name(value.value_type()));
        }
        QueryOperand::Input { .. } => {
            observed.operand_kinds.insert("input");
        }
    }
}

fn observe_pattern(
    pattern: &QueryPattern,
    local_functions: &BTreeSet<String>,
    observed: &mut ObservedPlanInventory,
) {
    match pattern {
        QueryPattern::Isa {
            include_subtypes,
            type_id,
            ..
        } => {
            observed.pattern_kinds.insert("isa");
            observed.isa_modes.insert(if *include_subtypes {
                "include_subtypes"
            } else {
                "exact"
            });
            let kind = query_builder_type_kind_name(type_id.kind())
                .expect("authored Isa types must belong to the queryable type-kind subset");
            observed.type_kinds.insert(kind);
        }
        QueryPattern::Has { .. } => {
            observed.pattern_kinds.insert("has");
        }
        QueryPattern::Links { relation_id, .. } => {
            observed.pattern_kinds.insert("links");
            observed.type_kinds.insert(
                query_builder_type_kind_name(relation_id.kind())
                    .expect("links relation must use one queryable type kind"),
            );
        }
        QueryPattern::Value {
            comparator,
            left,
            right,
        } => {
            observed.pattern_kinds.insert("value");
            observed
                .comparators
                .insert(query_builder_comparator_name(*comparator));
            observe_operand(left, observed);
            observe_operand(right, observed);
        }
        QueryPattern::Or { branches } => {
            observed.pattern_kinds.insert("or");
            for branch in branches {
                for pattern in branch {
                    observe_pattern(pattern, local_functions, observed);
                }
            }
        }
        QueryPattern::Not { patterns } => {
            observed.pattern_kinds.insert("not");
            for pattern in patterns {
                observe_pattern(pattern, local_functions, observed);
            }
        }
        QueryPattern::Try { patterns } => {
            observed.pattern_kinds.insert("try");
            for pattern in patterns {
                observe_pattern(pattern, local_functions, observed);
            }
        }
        QueryPattern::Reachable { relation, .. } => {
            observed.pattern_kinds.insert("reachable");
            observed.type_kinds.insert(
                query_builder_type_kind_name(relation.kind())
                    .expect("reachable relation must use one queryable type kind"),
            );
        }
        QueryPattern::FunctionCall {
            arguments,
            function,
            ..
        } => {
            observed.pattern_kinds.insert("function_call");
            observed.function_target_kinds.insert(
                if local_functions.contains(function.label().as_str()) {
                    "local"
                } else {
                    "schema"
                },
            );
            for argument in arguments {
                observe_operand(argument, observed);
            }
        }
    }
}

fn observe_plan(plan: &QueryPlan, observed: &mut ObservedPlanInventory) {
    let local_functions = plan
        .functions()
        .iter()
        .map(|function| function.name().label().as_str().to_owned())
        .collect::<BTreeSet<_>>();
    for input in plan.inputs() {
        observed
            .value_types
            .insert(query_builder_value_type_name(input.value_type()));
    }
    for function in plan.functions() {
        for pattern in function.body() {
            observe_pattern(pattern, &local_functions, observed);
        }
        observed
            .reducers
            .insert(query_builder_reducer_name(function.returns().reducer()));
        observed.value_types.insert(query_builder_value_type_name(
            function.returns().value_type(),
        ));
        observed.local_return_kinds.insert(format!(
            "{}:{}",
            query_builder_reducer_name(function.returns().reducer()),
            query_builder_value_type_name(function.returns().value_type())
        ));
    }
    for stage in plan.pipeline() {
        match stage {
            ReadStage::Match { patterns } => {
                observed.stage_kinds.insert("match");
                for pattern in patterns {
                    observe_pattern(pattern, &local_functions, observed);
                }
            }
            ReadStage::Select { .. } => {
                observed.stage_kinds.insert("select");
            }
            ReadStage::Require { .. } => {
                observed.stage_kinds.insert("require");
            }
            ReadStage::Distinct => {
                observed.stage_kinds.insert("distinct");
            }
            ReadStage::Reduce { assignments, .. } => {
                observed.stage_kinds.insert("reduce");
                for assignment in assignments {
                    observed
                        .reducers
                        .insert(query_builder_reducer_name(assignment.reducer()));
                }
            }
            ReadStage::Sort { terms } => {
                observed.stage_kinds.insert("sort");
                for term in terms {
                    observed
                        .order_directions
                        .insert(query_builder_order_direction_name(term.direction()));
                }
            }
            ReadStage::Offset { .. } => {
                observed.stage_kinds.insert("offset");
            }
            ReadStage::Limit { .. } => {
                observed.stage_kinds.insert("limit");
            }
        }
    }
    match plan.output() {
        QueryOutput::Rows { .. } => {
            observed.output_kinds.insert("rows");
        }
        QueryOutput::Documents { fields } => {
            observed.output_kinds.insert("documents");
            for field in fields {
                match field.source() {
                    DocumentSource::Binding { .. } => {
                        observed.document_source_kinds.insert("binding");
                    }
                    DocumentSource::AttributeList { .. } => {
                        observed.document_source_kinds.insert("attribute_list");
                    }
                }
            }
        }
    }
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("inventory type id")
}

fn inventory_declared_bytes() -> Vec<u8> {
    let person = type_id(TypeKind::Entity, "inventory-person");
    let edge = type_id(TypeKind::Relation, "inventory-edge");
    let origin = RoleId::new("inventory-edge", "origin").expect("origin role");
    let destination = RoleId::new("inventory-edge", "destination").expect("destination role");
    let scalar_attributes = [
        ("inventory-name", ValueTypeTag::String),
        ("inventory-age", ValueTypeTag::Long),
        ("inventory-ratio", ValueTypeTag::Double),
        ("inventory-active", ValueTypeTag::Boolean),
        ("inventory-born", ValueTypeTag::Date),
        ("inventory-seen", ValueTypeTag::DateTime),
        ("inventory-zoned", ValueTypeTag::DateTimeTz),
        ("inventory-amount", ValueTypeTag::Decimal),
        ("inventory-elapsed", ValueTypeTag::Duration),
    ];
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("person type")),
        SchemaFact::Type(TypeFact::new(edge.clone()).expect("edge type")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), origin.clone()).expect("origin relates"),
                None,
            )
            .expect("origin relates fact"),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), destination.clone()).expect("destination relates"),
                None,
            )
            .expect("destination relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), origin).expect("origin plays"),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), destination).expect("destination plays"),
        )),
    ];
    for (label, value_type) in scalar_attributes {
        let attribute = AttributeId::new(label).expect("inventory attribute");
        facts.push(SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, label)).expect("attribute type"),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute.clone()),
            value_type,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), attribute).expect("owns id"),
        )));
    }
    facts.push(SchemaFact::Function(FunctionFact::new(
        FunctionId::new("inventory_schema_mix").expect("schema function"),
        FunctionSignature::new(
            vec![
                FunctionParameter::new(
                    Label::new("subject").expect("subject parameter"),
                    TypeReference::Schema(
                        Label::new("inventory-person").expect("person reference"),
                    ),
                ),
                FunctionParameter::new(
                    Label::new("prefix").expect("prefix parameter"),
                    TypeReference::Value(ValueTypeTag::String),
                ),
                FunctionParameter::new(
                    Label::new("suffix").expect("suffix parameter"),
                    TypeReference::Value(ValueTypeTag::String),
                ),
            ],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("schema function signature"),
        FunctionBody::new("match $subject has inventory-name $name; return count($name);")
            .expect("schema function body"),
    )));
    facts.push(SchemaFact::Function(FunctionFact::new(
        FunctionId::new("inventory_long_identity").expect("identity function"),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("value").expect("value parameter"),
                TypeReference::Value(ValueTypeTag::Long),
            )],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("identity function signature"),
        FunctionBody::new("match let $result = $value; return first $result;")
            .expect("identity function body"),
    )));
    facts.push(SchemaFact::Function(FunctionFact::new(
        FunctionId::new("inventory_optional_long").expect("optional function"),
        FunctionSignature::new(
            Vec::new(),
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                true,
            )),
        )
        .expect("optional function signature"),
        FunctionBody::new("match let $value = 1; return first $value;")
            .expect("optional function body"),
    )));

    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("source byte");
        let line = u32::try_from(index + 1).expect("source line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-authoring-inventory").expect("document id"),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .expect("source span"),
        )
    });
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("inventory declared schema");
    encode_declared_schema(&declared).expect("inventory declared bytes")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures")
        .join(name)
}

fn repo_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(name)
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let mut bytes = fs::read(fixture_path(name)).expect("inventory fixture bytes");
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

fn corpus() -> serde_json::Value {
    serde_json::from_slice(
        &fs::read(fixture_path("query-v2-authoring-inventory.json"))
            .expect("inventory corpus bytes"),
    )
    .expect("inventory corpus JSON")
}

fn inventory_authority() -> Arc<QueryAuthority> {
    Arc::new(
        QueryAuthority::from_declared_bytes(
            &inventory_declared_bytes(),
            "query-v2-authoring-inventory",
            "typedb-3.12.1/v1",
        )
        .expect("inventory query authority"),
    )
}

#[derive(Clone, Debug)]
enum InventoryHandle {
    Binding(QueryBindingHandle),
    Input(QueryInputHandle),
    Operand(QueryOperandHandle),
    Pattern(QueryPatternHandle),
    Order(QueryOrderHandle),
    ReduceAssignment(QueryReduceAssignmentHandle),
    LocalReturn(QueryLocalReturnHandle),
    LocalFunction(QueryLocalFunctionHandle),
    DocumentField(QueryDocumentFieldHandle),
    Plan(Box<AuthoredQueryPlan>),
}

fn text<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("inventory field {key} must be text"))
}

fn optional_text<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    match value.get(key) {
        None | Some(serde_json::Value::Null) => None,
        Some(field) => Some(
            field
                .as_str()
                .unwrap_or_else(|| panic!("inventory field {key} must be text or null")),
        ),
    }
}

fn flag(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| panic!("inventory field {key} must be boolean"))
}

fn unsigned(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("inventory field {key} must be unsigned"))
}

fn text_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    let field = value
        .get(key)
        .unwrap_or_else(|| panic!("inventory field {key} is required"));
    if let Some(repetition) = field.as_object() {
        let repeated = repetition
            .get("repeat")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("inventory field {key}.repeat must be text"));
        let count = repetition
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or_else(|| panic!("inventory field {key}.count must be an unsigned size"));
        return vec![repeated.to_owned(); count];
    }
    field
        .as_array()
        .unwrap_or_else(|| panic!("inventory field {key} must be an array or repetition"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("inventory field {key} entries must be text"))
                .to_owned()
        })
        .collect()
}

fn binding(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryBindingHandle {
    match handles.get(name) {
        Some(InventoryHandle::Binding(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a binding"),
    }
}

fn input(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryInputHandle {
    match handles.get(name) {
        Some(InventoryHandle::Input(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not an input"),
    }
}

fn operand(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryOperandHandle {
    match handles.get(name) {
        Some(InventoryHandle::Operand(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not an operand"),
    }
}

fn pattern(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryPatternHandle {
    match handles.get(name) {
        Some(InventoryHandle::Pattern(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a pattern"),
    }
}

fn order(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryOrderHandle {
    match handles.get(name) {
        Some(InventoryHandle::Order(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not an order term"),
    }
}

fn reduce_assignment(
    handles: &BTreeMap<String, InventoryHandle>,
    name: &str,
) -> QueryReduceAssignmentHandle {
    match handles.get(name) {
        Some(InventoryHandle::ReduceAssignment(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a reduce assignment"),
    }
}

fn local_return(handles: &BTreeMap<String, InventoryHandle>, name: &str) -> QueryLocalReturnHandle {
    match handles.get(name) {
        Some(InventoryHandle::LocalReturn(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a local return"),
    }
}

fn local_function(
    handles: &BTreeMap<String, InventoryHandle>,
    name: &str,
) -> QueryLocalFunctionHandle {
    match handles.get(name) {
        Some(InventoryHandle::LocalFunction(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a local function"),
    }
}

fn document_field(
    handles: &BTreeMap<String, InventoryHandle>,
    name: &str,
) -> QueryDocumentFieldHandle {
    match handles.get(name) {
        Some(InventoryHandle::DocumentField(handle)) => handle.clone(),
        _ => panic!("inventory handle {name} is not a document field"),
    }
}

fn bindings(
    handles: &BTreeMap<String, InventoryHandle>,
    value: &serde_json::Value,
    key: &str,
) -> Vec<QueryBindingHandle> {
    text_array(value, key)
        .iter()
        .map(|name| binding(handles, name))
        .collect()
}

fn operands(
    handles: &BTreeMap<String, InventoryHandle>,
    value: &serde_json::Value,
    key: &str,
) -> Vec<QueryOperandHandle> {
    text_array(value, key)
        .iter()
        .map(|name| operand(handles, name))
        .collect()
}

fn patterns(
    handles: &BTreeMap<String, InventoryHandle>,
    value: &serde_json::Value,
    key: &str,
) -> Vec<QueryPatternHandle> {
    text_array(value, key)
        .iter()
        .map(|name| pattern(handles, name))
        .collect()
}

fn canonical_value(
    value_type_name: &str,
    value: &serde_json::Value,
) -> Result<CanonicalValue, Diagnostic> {
    Ok(match value_type_name {
        "string" => CanonicalValue::String(CanonicalString::new(
            value.as_str().expect("inventory string scalar"),
        )?),
        "long" => CanonicalValue::Long(
            value
                .as_str()
                .expect("inventory long spelling")
                .parse()
                .expect("inventory long"),
        ),
        "double" => CanonicalValue::Double(CanonicalDouble::new(
            value.as_f64().expect("inventory double"),
        )?),
        "boolean" => CanonicalValue::Boolean(value.as_bool().expect("inventory boolean")),
        "date" => CanonicalValue::Date(
            value
                .as_str()
                .expect("inventory date")
                .parse::<CanonicalDate>()?,
        ),
        "datetime" => CanonicalValue::DateTime(
            value
                .as_str()
                .expect("inventory datetime")
                .parse::<CanonicalDateTime>()?,
        ),
        "datetime_tz" => CanonicalValue::DateTimeTz(
            value
                .as_str()
                .expect("inventory datetime-tz")
                .parse::<CanonicalDateTimeTz>()?,
        ),
        "decimal" => CanonicalValue::Decimal(DecimalValue::new(
            value.as_str().expect("inventory decimal"),
        )?),
        "duration" => CanonicalValue::Duration(
            value
                .as_str()
                .expect("inventory duration")
                .parse::<CanonicalDuration>()?,
        ),
        _ => panic!("unknown inventory scalar type {value_type_name}"),
    })
}

fn execute_step(
    builder: &mut QueryPlanBuilder,
    handles: &mut BTreeMap<String, InventoryHandle>,
    step: &serde_json::Value,
) -> Result<Option<InventoryHandle>, Diagnostic> {
    let result = match text(step, "op") {
        "binding" => Some(InventoryHandle::Binding(
            builder.binding(text(step, "name"))?,
        )),
        "input" => Some(InventoryHandle::Input(builder.input(
            text(step, "name"),
            query_builder_value_type(text(step, "value_type"))?,
            flag(step, "optional"),
        )?)),
        "binding_operand" => Some(InventoryHandle::Operand(
            builder.binding_operand(&binding(handles, text(step, "binding")))?,
        )),
        "literal_operand" => Some(InventoryHandle::Operand(builder.literal_operand(
            canonical_value(
                text(step, "value_type"),
                step.get("value").expect("literal value"),
            )?,
        )?)),
        "input_operand" => Some(InventoryHandle::Operand(
            builder.input_operand(&input(handles, text(step, "input")))?,
        )),
        "isa" => {
            let kind = query_builder_type_kind(text(step, "type_kind"))?;
            Some(InventoryHandle::Pattern(builder.isa(
                &binding(handles, text(step, "binding")),
                TypeId::new(kind, text(step, "type_label"))?,
                flag(step, "include_subtypes"),
            )?))
        }
        "has" => Some(InventoryHandle::Pattern(builder.has(
            &binding(handles, text(step, "owner")),
            &binding(handles, text(step, "attribute")),
            AttributeId::new(text(step, "attribute_label"))?,
        )?)),
        "links" => {
            let relation_label = text(step, "relation_label");
            let players = query_builder_role_players(
                relation_label,
                text_array(step, "roles"),
                bindings(handles, step, "players"),
            )?;
            Some(InventoryHandle::Pattern(builder.links(
                &binding(handles, text(step, "relation")),
                TypeId::new(TypeKind::Relation, relation_label)?,
                players,
            )?))
        }
        "value" => Some(InventoryHandle::Pattern(builder.value(
            query_builder_comparator(text(step, "comparator"))?,
            &operand(handles, text(step, "left")),
            &operand(handles, text(step, "right")),
        )?)),
        "not" => {
            let repeat = step
                .get("repeat")
                .map(|_| unsigned(step, "repeat"))
                .unwrap_or(1);
            assert!(repeat > 0, "inventory negation repeat must be positive");
            let mut children = patterns(handles, step, "patterns");
            let mut nested = None;
            for _ in 0..repeat {
                let pattern = builder.not(children)?;
                children = vec![pattern.clone()];
                nested = Some(pattern);
            }
            Some(InventoryHandle::Pattern(
                nested.expect("positive negation repeat"),
            ))
        }
        "or" => {
            let branches = step
                .get("branches")
                .and_then(serde_json::Value::as_array)
                .expect("inventory branches")
                .iter()
                .map(|branch| {
                    branch
                        .as_array()
                        .expect("inventory branch")
                        .iter()
                        .map(|name| {
                            pattern(handles, name.as_str().expect("inventory pattern name"))
                        })
                        .collect()
                })
                .collect();
            Some(InventoryHandle::Pattern(builder.or(branches)?))
        }
        "try" => Some(InventoryHandle::Pattern(
            builder.r#try(patterns(handles, step, "patterns"))?,
        )),
        "reachable" => {
            let relation = text(step, "relation_label");
            Some(InventoryHandle::Pattern(builder.reachable(
                &binding(handles, text(step, "source")),
                &binding(handles, text(step, "target")),
                TypeId::new(TypeKind::Relation, relation)?,
                RoleId::new(relation, text(step, "role_from"))?,
                RoleId::new(relation, text(step, "role_to"))?,
                u8::try_from(unsigned(step, "min_depth")).expect("minimum depth"),
                u8::try_from(unsigned(step, "max_depth")).expect("maximum depth"),
            )?))
        }
        "function_call" => {
            let schema = optional_text(step, "function_name")
                .map(FunctionId::new)
                .transpose()?;
            let local =
                optional_text(step, "local_function").map(|name| local_function(handles, name));
            let target = query_builder_function_target(schema, local.as_ref())?;
            Some(InventoryHandle::Pattern(builder.function_call(
                &binding(handles, text(step, "assigned")),
                target,
                operands(handles, step, "arguments"),
            )?))
        }
        "order" => {
            let direction = query_builder_order_direction(text(step, "direction"))?;
            Some(InventoryHandle::Order(builder.order(
                &binding(handles, text(step, "binding")),
                direction,
            )?))
        }
        "reduce_assignment" => {
            let input = optional_text(step, "input").map(|name| binding(handles, name));
            Some(InventoryHandle::ReduceAssignment(
                builder.reduce_assignment(
                    &binding(handles, text(step, "assigned")),
                    query_builder_reducer(text(step, "reducer"))?,
                    input.as_ref(),
                )?,
            ))
        }
        "local_return" => Some(InventoryHandle::LocalReturn(builder.local_return(
            query_builder_reducer(text(step, "reducer"))?,
            &binding(handles, text(step, "input")),
            query_builder_value_type(text(step, "value_type"))?,
        )?)),
        "local_function" => {
            let parameters = query_builder_local_parameters(
                bindings(handles, step, "parameter_bindings"),
                text_array(step, "parameter_labels"),
            )?;
            Some(InventoryHandle::LocalFunction(builder.local_function(
                FunctionId::new(text(step, "name"))?,
                bindings(handles, step, "bindings"),
                parameters,
                patterns(handles, step, "body"),
                &local_return(handles, text(step, "returns")),
            )?))
        }
        "match" => {
            builder.r#match(patterns(handles, step, "patterns"))?;
            None
        }
        "select" => {
            builder.select(bindings(handles, step, "bindings"))?;
            None
        }
        "require" => {
            builder.require(bindings(handles, step, "bindings"))?;
            None
        }
        "distinct" => {
            builder.distinct()?;
            None
        }
        "reduce" => {
            let assignments = text_array(step, "assignments")
                .iter()
                .map(|name| reduce_assignment(handles, name))
                .collect();
            builder.reduce(assignments, bindings(handles, step, "groups"))?;
            None
        }
        "sort" => {
            let terms = text_array(step, "terms")
                .iter()
                .map(|name| order(handles, name))
                .collect();
            builder.sort(terms)?;
            None
        }
        "offset" => {
            builder.offset(unsigned(step, "rows"))?;
            None
        }
        "limit" => {
            builder.limit(unsigned(step, "rows"))?;
            None
        }
        "document_binding" => Some(InventoryHandle::DocumentField(
            builder
                .document_binding(text(step, "key"), &binding(handles, text(step, "binding")))?,
        )),
        "document_attribute_list" => Some(InventoryHandle::DocumentField(
            builder.document_attribute_list(
                text(step, "key"),
                &binding(handles, text(step, "owner")),
                AttributeId::new(text(step, "attribute_label"))?,
            )?,
        )),
        "finalize_rows" => Some(InventoryHandle::Plan(Box::new(
            builder.finalize_rows(bindings(handles, step, "bindings"))?,
        ))),
        "finalize_documents" => {
            let fields = text_array(step, "fields")
                .iter()
                .map(|name| document_field(handles, name))
                .collect();
            Some(InventoryHandle::Plan(Box::new(
                builder.finalize_documents(fields)?,
            )))
        }
        unknown => panic!("unknown inventory operation {unknown}"),
    };
    if let Some(id) = step.get("id").and_then(serde_json::Value::as_str) {
        handles.insert(
            id.to_owned(),
            result
                .clone()
                .unwrap_or_else(|| panic!("inventory operation {id} returned no handle")),
        );
    }
    Ok(result)
}

fn execute_plan(
    authority: Arc<QueryAuthority>,
    case: &serde_json::Value,
) -> Result<AuthoredQueryPlan, Diagnostic> {
    let mut builder = QueryPlanBuilder::new(authority);
    let mut handles = BTreeMap::new();
    for step in case
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .expect("inventory plan steps")
    {
        execute_step(&mut builder, &mut handles, step)?;
    }
    match handles.remove("plan") {
        Some(InventoryHandle::Plan(plan)) => Ok(*plan),
        _ => panic!("inventory case has no finalized plan"),
    }
}

fn invocation_rows(
    value: &serde_json::Value,
) -> Result<Vec<Vec<Option<CanonicalValue>>>, Diagnostic> {
    value
        .as_array()
        .expect("inventory invocation rows")
        .iter()
        .map(|row| {
            row.as_array()
                .expect("inventory invocation row")
                .iter()
                .map(|cell| {
                    if cell.is_null() {
                        Ok(None)
                    } else {
                        Ok(Some(canonical_value(
                            text(cell, "type"),
                            cell.get("value").expect("inventory invocation value"),
                        )?))
                    }
                })
                .collect()
        })
        .collect()
}

fn invoke(
    plan: &AuthoredQueryPlan,
    terminal: &str,
    rows: &serde_json::Value,
) -> Result<AuthoredQueryInvocation, Diagnostic> {
    let rows = invocation_rows(rows)?;
    match terminal {
        "rows" => plan.rows(rows),
        "documents" => plan.documents(rows),
        "count" => plan.count(rows),
        "exists" => plan.exists(rows),
        unknown => panic!("unknown inventory terminal {unknown}"),
    }
}

fn expected_bytes(value: &serde_json::Value) -> Vec<u8> {
    BASE64
        .decode(text(value, "canonical_b64"))
        .expect("inventory canonical base64")
}

fn capability_names(capabilities: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    capabilities
        .into_iter()
        .map(|capability| capability.as_ref().to_owned())
        .collect()
}

fn assert_unique_manifest<T: std::fmt::Debug>(name: &str, manifest: &[(T, &str)]) {
    assert_eq!(
        manifest.len(),
        manifest
            .iter()
            .map(|(variant, _)| format!("{variant:?}"))
            .collect::<BTreeSet<_>>()
            .len(),
        "{name} repeats a source variant"
    );
    assert_eq!(
        manifest.len(),
        manifest
            .iter()
            .map(|(_, spelling)| *spelling)
            .collect::<BTreeSet<_>>()
            .len(),
        "{name} repeats a public spelling"
    );
}

fn source_public_builder_operations() -> BTreeSet<String> {
    let source = include_str!("../src/query_v2_builder.rs");
    let implementation = source
        .split_once("impl QueryPlanBuilder {")
        .expect("QueryPlanBuilder implementation")
        .1
        .split_once("\nfn collect_operand_binding")
        .expect("end of QueryPlanBuilder implementation")
        .0;
    implementation
        .lines()
        .filter_map(|line| {
            let declaration = line
                .trim()
                .strip_prefix("pub fn ")
                .or_else(|| line.trim().strip_prefix("pub const fn "))?;
            let name = declaration.split_once('(')?.0.strip_prefix("r#").unwrap_or(
                declaration
                    .split_once('(')
                    .expect("public function declaration")
                    .0,
            );
            (!matches!(name, "new" | "finalized_plan")).then(|| name.to_owned())
        })
        .collect()
}

fn snake_case_variant(name: &str) -> String {
    let mut spelling = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                spelling.push('_');
            }
            spelling.push(character.to_ascii_lowercase());
        } else {
            spelling.push(character);
        }
    }
    spelling
}

fn source_contract_enum_spellings(name: &str) -> BTreeSet<String> {
    let source = include_str!("../../contract/src/query_plan.rs");
    let marker = format!("pub enum {name} {{");
    let declaration = source
        .split_once(&marker)
        .unwrap_or_else(|| panic!("contract enum {name}"))
        .1;
    let mut depth = 1isize;
    let mut variants = BTreeSet::new();
    for line in declaration.lines() {
        let trimmed = line.trim();
        if depth == 1
            && trimmed
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_uppercase())
        {
            let variant = trimmed
                .split(|character: char| !character.is_ascii_alphanumeric())
                .next()
                .expect("enum variant");
            assert!(
                variants.insert(snake_case_variant(variant)),
                "contract enum {name} repeats variant {variant}"
            );
        }
        depth += line.chars().filter(|character| *character == '{').count() as isize;
        depth -= line.chars().filter(|character| *character == '}').count() as isize;
        if depth == 0 {
            break;
        }
    }
    assert!(!variants.is_empty(), "contract enum {name} has no variants");
    variants
}

fn source_authoring_diagnostic_codes() -> BTreeSet<String> {
    let authoring_sources = [
        include_str!("../src/query_v2_builder.rs"),
        include_str!("../../query/src/query_validation.rs"),
        include_str!("../../python/src/query_v2_builder_runtime.rs"),
        include_str!("../../node/src/query_v2_builder_runtime.rs"),
    ];
    authoring_sources
        .into_iter()
        .flat_map(|source| {
            source
                .split_once("\n#[cfg(test)]")
                .map_or(source, |(production, _)| production)
                .split('"')
        })
        .filter(|candidate| {
            ["query_builder_", "query_plan_", "query_invocation_"]
                .iter()
                .any(|prefix| candidate.starts_with(prefix))
        })
        .chain(
            include_str!("../src/query_v2_prepared.rs")
                .split_once("\n#[cfg(test)]")
                .map_or(
                    include_str!("../src/query_v2_prepared.rs"),
                    |(production, _)| production,
                )
                .split('"')
                .filter(|candidate| candidate.starts_with("query_v2_host_")),
        )
        .filter(|candidate| {
            candidate.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn rust_authority_recomputes_and_decodes_every_shared_vector() {
    let declared_fixture = fixture_bytes("query-v2-authoring-inventory-declared.json");
    assert_eq!(inventory_declared_bytes(), declared_fixture);

    assert_unique_manifest("value types", QUERY_BUILDER_VALUE_TYPES);
    assert_unique_manifest("type kinds", QUERY_BUILDER_TYPE_KINDS);
    assert_unique_manifest("comparators", QUERY_BUILDER_COMPARATORS);
    assert_unique_manifest("order directions", QUERY_BUILDER_ORDER_DIRECTIONS);
    assert_unique_manifest("reducers", QUERY_BUILDER_REDUCERS);

    let corpus = corpus();
    assert_eq!(
        text(&corpus["authority"], "vector_producer"),
        "type-bridge-core/crates/orm/tests/query_v2_authoring_inventory.rs"
    );
    let inventory_operations = corpus["inventory"]["builder_operations"]
        .as_array()
        .expect("inventory operations")
        .iter()
        .map(|operation| operation.as_str().expect("operation name"))
        .collect::<Vec<_>>();
    assert_eq!(QUERY_PLAN_BUILDER_OPERATIONS, inventory_operations);
    assert_eq!(
        source_public_builder_operations(),
        QUERY_PLAN_BUILDER_OPERATIONS
            .iter()
            .map(|operation| (*operation).to_owned())
            .collect(),
        "the operation manifest must equal the actual public semantic QueryPlanBuilder methods"
    );

    let authority = inventory_authority();
    let mut observed = ObservedPlanInventory::default();
    let mut observed_capabilities = BTreeSet::new();
    let mut observed_transport_capabilities = BTreeSet::new();
    let mut observed_transport_variants = BTreeSet::new();
    let mut observed_terminals = BTreeSet::new();
    let mut observed_operations = BTreeSet::new();
    for case in corpus["plans"].as_array().expect("inventory plans") {
        let case_id = text(case, "id");
        for step in case["steps"].as_array().expect("inventory plan steps") {
            observed_operations.insert(text(step, "op").to_owned());
        }
        let authored = execute_plan(Arc::clone(&authority), case)
            .unwrap_or_else(|error| panic!("{case_id}: {error}"));
        let expected = case.get("expected").expect("expected plan vector");
        let expected_plan_bytes = expected_bytes(expected);
        assert_eq!(
            authored.canonical_bytes(),
            expected_plan_bytes,
            "{case_id}: authored plan bytes"
        );
        assert_eq!(
            authored.fingerprint_hex(),
            text(expected, "fingerprint"),
            "{case_id}: authored fingerprint"
        );

        let decoded = decode_query_plan(&expected_plan_bytes)
            .unwrap_or_else(|error| panic!("{case_id}: vector decode: {error}"));
        observe_plan(&decoded, &mut observed);
        observed_capabilities.extend(
            decoded
                .required_capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned()),
        );
        assert_eq!(
            decoded.canonical_bytes().expect("decoded plan bytes"),
            expected_plan_bytes,
            "{case_id}: decoded canonical round trip"
        );
        assert_eq!(
            decoded
                .fingerprint()
                .expect("decoded plan fingerprint")
                .as_fingerprint()
                .digest()
                .to_hex(),
            text(expected, "fingerprint"),
            "{case_id}: decoded fingerprint"
        );
        assert_eq!(
            authored.required_capabilities(),
            capability_names(
                decoded
                    .required_capabilities()
                    .iter()
                    .map(|capability| capability.as_str())
            ),
            "{case_id}: capability derivation"
        );

        for invocation_case in case["invocations"]
            .as_array()
            .expect("inventory invocations")
        {
            let invocation_id = text(invocation_case, "id");
            observed_terminals.insert(text(invocation_case, "terminal").to_owned());
            let authored_invocation = invoke(
                &authored,
                text(invocation_case, "terminal"),
                invocation_case.get("rows").expect("invocation rows"),
            )
            .unwrap_or_else(|error| panic!("{case_id}/{invocation_id}: {error}"));
            let expected_invocation = invocation_case
                .get("expected")
                .expect("expected invocation vector");
            let expected_invocation_bytes = expected_bytes(expected_invocation);
            assert_eq!(
                authored_invocation.canonical_bytes(),
                expected_invocation_bytes,
                "{case_id}/{invocation_id}: authored invocation bytes"
            );

            let decoded_invocation = decode_query_invocation(&decoded, &expected_invocation_bytes)
                .unwrap_or_else(|error| {
                    panic!("{case_id}/{invocation_id}: vector decode: {error}")
                });
            if decoded_invocation.inputs().len() == 1
                && decoded_invocation.inputs()[0]
                    .values()
                    .iter()
                    .flatten()
                    .any(|value| value.value_type() == ValueTypeTag::DateTimeTz)
            {
                observed_transport_variants.insert("datetime_tz_single");
            }
            if decoded_invocation.inputs().len() > 1 {
                observed_transport_variants.insert("multirow");
            }
            if decoded_invocation.inputs().iter().any(|row| {
                decoded
                    .inputs()
                    .iter()
                    .zip(row.values())
                    .any(|(column, value)| column.optional() && value.is_none())
            }) {
                observed_transport_variants.insert("optional_null");
            }
            assert_eq!(
                to_canonical_json(&decoded_invocation).expect("decoded invocation bytes"),
                expected_invocation_bytes,
                "{case_id}/{invocation_id}: decoded canonical round trip"
            );
            assert_eq!(
                authored_invocation.operation(),
                decoded_invocation.operation(),
                "{case_id}/{invocation_id}: operation"
            );
            assert_eq!(
                authored_invocation.plan_fingerprint(),
                decoded_invocation.plan_fingerprint(),
                "{case_id}/{invocation_id}: plan binding"
            );
            assert_eq!(
                authored_invocation.required_transport_capabilities(),
                capability_names(
                    decoded_invocation
                        .transport_capabilities()
                        .iter()
                        .map(|capability| capability.as_str())
                ),
                "{case_id}/{invocation_id}: transport capabilities"
            );
            assert_eq!(
                authored_invocation.required_transport_capabilities(),
                &text_array(expected_invocation, "required_transport_capabilities"),
                "{case_id}/{invocation_id}: frozen transport capabilities"
            );
            observed_transport_capabilities.extend(
                authored_invocation
                    .required_transport_capabilities()
                    .iter()
                    .cloned(),
            );
        }
    }

    for case in corpus["diagnostics"]
        .as_array()
        .expect("inventory diagnostics")
    {
        for step in case
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            observed_operations.insert(text(step, "op").to_owned());
        }
        if let Some(failure) = case.get("failure") {
            observed_operations.insert(text(failure, "op").to_owned());
        }
    }

    let expected_operations = QUERY_PLAN_BUILDER_OPERATIONS
        .iter()
        .map(|operation| (*operation).to_owned())
        .collect();
    assert_eq!(
        observed_operations, expected_operations,
        "every source-owned builder operation must be exercised by the corpus"
    );

    let expected_capabilities = query_plan_authoring_capability_vocabulary()
        .iter()
        .map(|capability| capability.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        observed_capabilities, expected_capabilities,
        "the authored plan union must derive the complete low-level capability vocabulary"
    );

    assert_eq!(
        observed_transport_capabilities,
        BTreeSet::from([query_given_rows_capability().as_str().to_owned()]),
        "the invocation union must derive the complete transport capability vocabulary"
    );

    assert_eq!(
        observed.type_kinds,
        QUERY_BUILDER_TYPE_KINDS
            .iter()
            .map(|(kind, spelling)| {
                assert_eq!(query_builder_type_kind_name(*kind), Some(*spelling));
                *spelling
            })
            .collect(),
        "the decoded corpus must exercise every queryable schema kind"
    );
    assert_eq!(
        observed.value_types,
        QUERY_BUILDER_VALUE_TYPES
            .iter()
            .map(|(value_type, spelling)| {
                assert_eq!(query_builder_value_type_name(*value_type), *spelling);
                *spelling
            })
            .collect(),
        "the decoded corpus must exercise every canonical scalar domain"
    );
    assert_eq!(
        observed.comparators,
        QUERY_BUILDER_COMPARATORS
            .iter()
            .map(|(comparator, spelling)| {
                assert_eq!(query_builder_comparator_name(*comparator), *spelling);
                *spelling
            })
            .collect(),
        "the decoded corpus must exercise every comparator"
    );
    assert_eq!(
        observed.reducers,
        QUERY_BUILDER_REDUCERS
            .iter()
            .map(|(reducer, spelling)| {
                assert_eq!(query_builder_reducer_name(*reducer), *spelling);
                *spelling
            })
            .collect(),
        "the decoded corpus must exercise every reducer"
    );
    assert_eq!(
        observed
            .operand_kinds
            .iter()
            .map(|spelling| (*spelling).to_owned())
            .collect::<BTreeSet<_>>(),
        source_contract_enum_spellings("QueryOperand"),
        "the decoded corpus must exercise every operand kind"
    );
    assert_eq!(
        observed
            .pattern_kinds
            .iter()
            .map(|spelling| (*spelling).to_owned())
            .collect::<BTreeSet<_>>(),
        source_contract_enum_spellings("QueryPattern"),
        "the decoded corpus must exercise every pattern kind"
    );
    assert_eq!(
        observed
            .stage_kinds
            .iter()
            .map(|spelling| (*spelling).to_owned())
            .collect::<BTreeSet<_>>(),
        source_contract_enum_spellings("ReadStage"),
        "the decoded corpus must exercise every read-stage kind"
    );
    assert_eq!(
        observed
            .output_kinds
            .iter()
            .map(|spelling| (*spelling).to_owned())
            .collect::<BTreeSet<_>>(),
        source_contract_enum_spellings("QueryOutput"),
        "the decoded corpus must exercise every output kind"
    );
    assert_eq!(
        observed
            .document_source_kinds
            .iter()
            .map(|spelling| (*spelling).to_owned())
            .collect::<BTreeSet<_>>(),
        source_contract_enum_spellings("DocumentSource"),
        "the decoded corpus must exercise every document-source kind"
    );
    assert_eq!(
        observed.order_directions,
        QUERY_BUILDER_ORDER_DIRECTIONS
            .iter()
            .map(|(direction, spelling)| {
                assert_eq!(query_builder_order_direction_name(*direction), *spelling);
                *spelling
            })
            .collect(),
        "the decoded corpus must exercise every order direction"
    );
    assert_eq!(
        observed.isa_modes,
        ISA_MODES.iter().copied().collect(),
        "the decoded corpus must exercise exact and subtype-inclusive Isa"
    );
    assert_eq!(
        observed.function_target_kinds,
        FUNCTION_TARGET_KINDS.iter().copied().collect(),
        "the decoded corpus must exercise schema and local function targets"
    );
    assert_eq!(
        observed.local_return_kinds,
        QUERY_BUILDER_LOCAL_RETURNS
            .iter()
            .map(|(reducer, value_type)| {
                format!(
                    "{}:{}",
                    query_builder_reducer_name(*reducer),
                    query_builder_value_type_name(*value_type)
                )
            })
            .collect(),
        "the decoded corpus must exercise every legal local-return pair"
    );
    assert_eq!(
        observed_transport_variants,
        TRANSPORT_VARIANTS.iter().copied().collect(),
        "the decoded invocation corpus must exercise every transport subcase"
    );
    let mut expected_terminals = [
        QueryOperation::Rows,
        QueryOperation::Count,
        QueryOperation::Exists,
    ]
    .map(operation_name)
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    expected_terminals.extend(
        FACADE_TERMINALS
            .iter()
            .map(|terminal| (*terminal).to_owned()),
    );
    assert_eq!(
        observed_terminals, expected_terminals,
        "the corpus must invoke every public terminal"
    );
}

#[test]
fn rust_authority_preserves_every_shared_diagnostic_tuple() {
    let corpus = corpus();
    let authority = inventory_authority();
    let mut observed_diagnostic_kinds = BTreeSet::new();
    let mut observed_nonempty_details = false;
    let plans = corpus["plans"]
        .as_array()
        .expect("inventory plans")
        .iter()
        .map(|case| {
            (
                text(case, "id").to_owned(),
                execute_plan(Arc::clone(&authority), case).expect("inventory diagnostic plan"),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for case in corpus["diagnostics"]
        .as_array()
        .expect("inventory diagnostics")
    {
        let case_id = text(case, "id");
        let kind = text(case, "kind");
        observed_diagnostic_kinds.insert(kind);
        let error = match kind {
            "builder" => {
                let mut builder = QueryPlanBuilder::new(Arc::clone(&authority));
                let mut handles = BTreeMap::new();
                for step in case["steps"].as_array().expect("diagnostic steps") {
                    execute_step(&mut builder, &mut handles, step)
                        .unwrap_or_else(|error| panic!("{case_id} setup: {error}"));
                }
                execute_step(
                    &mut builder,
                    &mut handles,
                    case.get("failure").expect("diagnostic failure step"),
                )
                .expect_err("inventory builder diagnostic must fail")
            }
            "invocation" => invoke(
                plans.get(text(case, "plan")).expect("diagnostic plan"),
                text(case, "terminal"),
                case.get("rows").expect("diagnostic rows"),
            )
            .expect_err("inventory invocation diagnostic must fail"),
            unknown => panic!("unknown inventory diagnostic kind {unknown}"),
        };
        observed_nonempty_details |= !error.details().is_empty();
        let actual = serde_json::to_value(&error).expect("diagnostic JSON");
        assert_eq!(
            actual
                .as_object()
                .expect("diagnostic object")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["category", "code", "details", "message", "path"]),
            "{case_id}: complete structured diagnostic shape"
        );
        assert_eq!(
            actual,
            case.get("expected").expect("expected diagnostic").clone(),
            "{case_id}: complete diagnostic tuple"
        );
    }
    assert_eq!(
        observed_diagnostic_kinds,
        BTreeSet::from(["builder", "invocation"]),
        "the shared inventory must exercise both authoring failure boundaries"
    );
    assert!(
        observed_nonempty_details,
        "the shared inventory must preserve at least one typed diagnostic detail"
    );
}

#[test]
fn boundary_evidence_is_complete_and_source_tied() {
    let corpus = corpus();
    let diagnostic_ids = corpus["diagnostics"]
        .as_array()
        .expect("inventory diagnostics")
        .iter()
        .map(|case| text(case, "id").to_owned())
        .collect::<BTreeSet<_>>();
    let named_diagnostic_ids = text_array(&corpus["inventory"], "diagnostic_cases");
    assert_eq!(
        named_diagnostic_ids.len(),
        diagnostic_ids.len(),
        "the named diagnostic inventory must not contain duplicates"
    );
    assert_eq!(
        named_diagnostic_ids.into_iter().collect::<BTreeSet<_>>(),
        diagnostic_ids,
        "the named diagnostic inventory must exactly match the executable cases"
    );

    let evidence = &corpus["boundary_evidence"];
    let mut families = BTreeSet::new();
    let mut ledger_cases = BTreeSet::new();
    for entry in evidence["shared_parity"]
        .as_array()
        .expect("shared boundary evidence")
    {
        let family = text(entry, "family");
        assert!(
            families.insert(family.to_owned()),
            "duplicate boundary family {family}"
        );
        let cases = text_array(entry, "cases");
        assert!(!cases.is_empty(), "{family} has no shared boundary cases");
        for case in cases {
            assert!(
                diagnostic_ids.contains(&case),
                "{family} names unknown shared diagnostic {case}"
            );
            assert!(
                ledger_cases.insert(case.clone()),
                "shared diagnostic {case} is assigned to more than one boundary family"
            );
        }
    }
    assert_eq!(
        ledger_cases, diagnostic_ids,
        "every shared diagnostic must belong to exactly one boundary family"
    );

    for section in [
        "binding_tests",
        "rust_authority_tests",
        "source_guards",
        "direct_wire_only",
        "non_applicable",
    ] {
        let entries = evidence[section]
            .as_array()
            .unwrap_or_else(|| panic!("{section} boundary evidence"));
        assert!(!entries.is_empty(), "{section} must not be empty");
        for entry in entries {
            let family = text(entry, "family");
            assert!(
                families.insert(family.to_owned()),
                "duplicate boundary family {family}"
            );
            if matches!(section, "direct_wire_only" | "non_applicable") {
                assert!(
                    !text(entry, "reason").is_empty(),
                    "{family} must explain why public host parity is inapplicable"
                );
            }
            let path = text(entry, "path");
            let relative = PathBuf::from(path);
            assert!(
                relative.is_relative()
                    && !relative.components().any(|part| part.as_os_str() == ".."),
                "{family} evidence path must stay within the repository"
            );
            let source = fs::read_to_string(repo_path(path))
                .unwrap_or_else(|error| panic!("{family}: cannot read {path}: {error}"));
            let needles = text_array(entry, "needles");
            assert!(!needles.is_empty(), "{family} has no source-tied needles");
            for needle in needles {
                assert!(
                    source.contains(&needle),
                    "{family}: {path} no longer contains required evidence {needle:?}"
                );
            }
        }
    }

    let shared_codes = corpus["diagnostics"]
        .as_array()
        .expect("inventory diagnostics")
        .iter()
        .map(|case| text(&case["expected"], "code").to_owned())
        .collect::<BTreeSet<_>>();
    let source_codes = source_authoring_diagnostic_codes();
    let source_only_codes = source_codes
        .difference(&shared_codes)
        .cloned()
        .collect::<BTreeSet<_>>();
    let classifications = evidence["diagnostic_code_classification"]
        .as_array()
        .expect("diagnostic code classification");
    let mut classified_codes = BTreeSet::new();
    for entry in classifications {
        let code = text(entry, "code");
        assert!(
            source_codes.contains(code),
            "diagnostic classification names non-source code {code}"
        );
        assert!(
            !shared_codes.contains(code),
            "shared diagnostic {code} must not also be classified as a boundary exception"
        );
        assert!(
            classified_codes.insert(code.to_owned()),
            "diagnostic code {code} is classified more than once"
        );
        match text(entry, "class") {
            "binding_parity" => {
                let evidence_families = text_array(entry, "evidence_families");
                assert!(
                    evidence_families.len() >= 2,
                    "{code} binding parity requires both host projections or one explicit non-applicability proof"
                );
                assert!(
                    evidence_families
                        .iter()
                        .any(|family| family.starts_with("python_"))
                        && evidence_families
                            .iter()
                            .any(|family| family.starts_with("node_")),
                    "{code} binding parity must cite one Python and one Node family"
                );
                for family in evidence_families {
                    assert!(
                        families.contains(&family),
                        "{code} cites unknown boundary evidence family {family}"
                    );
                }
            }
            "direct_or_internal" => {
                assert!(
                    !text(entry, "reason").is_empty(),
                    "{code} direct/internal classification requires a rationale"
                );
                let path = text(entry, "path");
                let relative = PathBuf::from(path);
                assert!(
                    relative.is_relative()
                        && !relative.components().any(|part| part.as_os_str() == ".."),
                    "{code} classification path must stay within the repository"
                );
                let source = fs::read_to_string(repo_path(path))
                    .unwrap_or_else(|error| panic!("{code}: cannot read {path}: {error}"));
                let needles = text_array(entry, "needles");
                assert!(!needles.is_empty(), "{code} has no source-tied needles");
                for needle in needles {
                    assert!(
                        source.contains(&needle),
                        "{code}: {path} no longer contains required evidence {needle:?}"
                    );
                }
            }
            unknown => panic!("diagnostic {code} has unknown classification {unknown}"),
        }
    }
    assert_eq!(
        classified_codes, source_only_codes,
        "every source-reachable authoring diagnostic must be shared or exactly classified"
    );

    assert!(
        !diagnostic_ids.contains("invocation_value_type"),
        "tagged canonical scalar mismatches cannot be authored by untagged public host rows"
    );
}
