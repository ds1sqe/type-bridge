//! Persistent handle construction and deterministic canonical lowering.

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps, TxType,
};
use type_bridge_orm::*;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attribute(field_name: &str, attr_name: &str, value_type: ValueType) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.to_owned(),
        attr_name: attr_name.to_owned(),
        value_type,
        annotations: Vec::new(),
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                OwnedAttributeDescriptor {
                    annotations: vec![Annotation::Key],
                    ..attribute("name", "person-name", ValueType::String)
                },
                attribute("age", "person-age", ValueType::Long),
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "company".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                annotations: vec![Annotation::Key],
                ..attribute("name", "company-name", ValueType::String)
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attribute(
                "position",
                "employment-position",
                ValueType::String,
            )],
            roles: vec![
                RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "employer".into(),
                    player_type_names: vec!["company".into()],
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "node".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "leaf-node".into(),
            is_abstract: false,
            parent_type: Some("node".into()),
            owned_attributes: vec![],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "directed-edge".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![
                RoleDescriptor {
                    role_name: "origin".into(),
                    player_type_names: vec!["leaf-node".into()],
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "destination".into(),
                    player_type_names: vec!["leaf-node".into()],
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    let inherited_name = OwnedAttributeDescriptor {
        annotations: vec![Annotation::Key],
        ..attribute("name", "party-name", ValueType::String)
    };
    registry
        .register_entity(EntityDescriptor {
            type_name: "party".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![inherited_name.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "employee".into(),
            is_abstract: false,
            parent_type: Some("party".into()),
            owned_attributes: vec![inherited_name],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    let inherited_participant = RoleDescriptor {
        role_name: "participant".into(),
        player_type_names: vec!["person".into()],
        ..Default::default()
    };
    registry
        .register_relation(RelationDescriptor {
            type_name: "association".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![inherited_participant.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "special-association".into(),
            is_abstract: false,
            parent_type: Some("association".into()),
            owned_attributes: vec![],
            roles: vec![inherited_participant.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "specialized-association".into(),
            is_abstract: false,
            parent_type: Some("association".into()),
            owned_attributes: vec![],
            roles: vec![RoleDescriptor {
                role_name: "member".into(),
                player_type_names: vec!["person".into()],
                overrides: Some("participant".into()),
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "collaboration".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![inherited_participant],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
}

fn assert_match_code<T: Debug>(result: type_bridge_orm::Result<T>, expected: &str) {
    match result {
        Err(OrmError::Match(error)) => assert_eq!(error.code().as_str(), expected),
        other => panic!("expected match error {expected}, got {other:?}"),
    }
}

fn assert_function_value_package_mismatch<T: Debug>(result: type_bridge_orm::Result<T>) {
    let Err(OrmError::Match(error)) = result else {
        panic!("expected generated-token package mismatch");
    };
    assert_eq!(error.code().as_str(), "generated_token_package_mismatch");
    assert_eq!(
        error.message(),
        "The generated token belongs to a different installed schema package",
    );
    let diagnostic = lower_match_error(&error);
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        diagnostic.code().as_str(),
        "generated_token_package_mismatch",
    );
    assert_eq!(
        diagnostic.message().as_str(),
        "The generated token belongs to a different installed schema package",
    );
    assert!(diagnostic.path().is_empty());
    assert!(diagnostic.details().is_empty());
}

fn assert_send_sync<T: Send + Sync>() {}

const QUALIFYING_SCORE_BODY: &str = "match $person has score $score-attribute; let $score = $score-attribute; $score >= $minimum; return first $score;";

fn installed_function_projection() -> InstalledRuntimeProjection {
    installed_function_projection_with_body(QUALIFYING_SCORE_BODY)
}

fn installed_function_projection_with_body(body: &str) -> InstalledRuntimeProjection {
    installed_function_projection_with_evidence(body, BindingTarget::Python, &[])
}

fn installed_function_projection_with_evidence(
    body: &str,
    target: BindingTarget,
    resources: &[CodeResourceDigest],
) -> InstalledRuntimeProjection {
    let source = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  score: { value: integer }
entities:
  actor: { abstract: true }
  person:
    sub: actor
    owns:
      name: { key: true }
      score: { card: 1 }
  employee: { sub: person }
functions:
  qualifying-score:
    parameters:
      - { name: person, type: person }
      - { name: minimum, type: integer }
    returns: { scalar: integer }
    body: { typeql: "__QUALIFYING_SCORE_BODY__" }
  person-name:
    parameters:
      - { name: person, type: person }
    returns: { scalar: string }
    body: { typeql: "match $person has name $name; return first $name;" }
  person-score:
    parameters:
      - { name: person, type: person }
    returns: { scalar: integer }
    body: { typeql: "match $person has score $score-attribute; let $score = $score-attribute; return first $score;" }
  identity-score:
    parameters:
      - { name: score, type: integer }
    returns: { scalar: integer }
    body: { typeql: "match $score == $score; return first $score;" }
  find-people:
    parameters: []
    returns: { stream: [person] }
    body: { typeql: "match $person isa person; return { $person };" }
"#
    .replace("__QUALIFYING_SCORE_BODY__", body);
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("function-handles.yaml").unwrap(),
        source.as_str(),
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let (config, handlers) = match target {
        BindingTarget::Python => (
            ProjectionConfig::python(),
            vec![ProjectionHandler::python_v1()],
        ),
        BindingTarget::TypeScript => (
            ProjectionConfig::typescript(),
            vec![ProjectionHandler::typescript_v1()],
        ),
        _ => panic!("focused function handles need only Python and TypeScript projections"),
    };
    InstalledRuntimeProjection::try_new(
        project(&resolved, target, &config, &handlers, resources).unwrap(),
    )
    .unwrap()
}

fn installed_wide_function_projection(parameter_count: usize) -> InstalledRuntimeProjection {
    let parameters = (0..parameter_count)
        .map(|index| format!("      - {{ name: p{index}, type: integer }}\n"))
        .collect::<String>();
    let source = format!(
        "format: typebridge.schema/v2\nfunctions:\n  wide-function:\n    parameters:\n{parameters}    returns: {{ scalar: integer }}\n    body: {{ typeql: \"match let $out = 1; return first $out;\" }}\n"
    );
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("wide-function.yaml").unwrap(),
        source.as_str(),
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

#[derive(Default)]
struct FunctionProviderEvents {
    opens: AtomicUsize,
    plain: Mutex<Vec<String>>,
    given: Mutex<Vec<(String, GivenRowsSpec)>>,
}

struct FunctionBackend {
    events: Arc<FunctionProviderEvents>,
    given: bool,
    server_version: type_bridge_core_lib::version::Version,
}

impl DriverBackend for FunctionBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        CapabilitySet::from_iter(
            Capability::ALL
                .into_iter()
                .filter(|capability| self.given || *capability != Capability::SchemaFunctionCall),
        )
    }

    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, type_bridge_orm::Result<Box<dyn TransactionOps>>> {
        self.events.opens.fetch_add(1, AtomicOrdering::SeqCst);
        let transaction = FunctionTransaction {
            events: Arc::clone(&self.events),
            given: self.given,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        Some(self.server_version)
    }

    fn supports_given_rows(&self) -> bool {
        self.given
    }
}

struct FunctionTransaction {
    events: Arc<FunctionProviderEvents>,
    given: bool,
}

impl TransactionOps for FunctionTransaction {
    fn supports_given_rows(&self) -> bool {
        self.given
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, type_bridge_orm::Result<QueryResult>> {
        self.events.plain.lock().unwrap().push(typeql.to_owned());
        Box::pin(async { Ok(QueryResult::Rows(Vec::new())) })
    }

    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, type_bridge_orm::Result<QueryResult>> {
        self.events
            .given
            .lock()
            .unwrap()
            .push((typeql.to_owned(), rows));
        Box::pin(async { Ok(QueryResult::Rows(Vec::new())) })
    }

    fn commit(&mut self) -> BoxFuture<'_, type_bridge_orm::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn rollback(&mut self) -> BoxFuture<'_, type_bridge_orm::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, type_bridge_orm::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn function_exists_request(
    installed: &InstalledRuntimeProjection,
    registry: &Arc<DescriptorRegistry>,
) -> ValidatedMatchRequest {
    let session = SessionHandle::new(Arc::clone(registry));
    let person = session.exact("person").unwrap();
    let minimum = ProjectedAttributeValue::try_new(
        installed,
        TypeId::new(TypeKind::Attribute, "score").unwrap(),
        CanonicalValue::Long(40),
    )
    .unwrap();
    let minimum = session.function_value(&minimum).unwrap();
    let call = session
        .function(&FunctionId::new("qualifying-score").unwrap())
        .unwrap()
        .call([person.function_argument(), minimum.function_argument()])
        .unwrap();
    let request = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap()
        .where_predicate(
            call.compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
                .unwrap(),
        )
        .unwrap()
        .exists_by(&person)
        .unwrap();
    validate_match_request(registry, request).unwrap()
}

fn model_only_function_exists_request(registry: &Arc<DescriptorRegistry>) -> ValidatedMatchRequest {
    let session = SessionHandle::new(Arc::clone(registry));
    let person = session.exact("person").unwrap();
    let call = session
        .function(&FunctionId::new("person-score").unwrap())
        .unwrap()
        .call([person.function_argument()])
        .unwrap();
    let request = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap()
        .where_predicate(
            call.compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
                .unwrap(),
        )
        .unwrap()
        .exists_by(&person)
        .unwrap();
    validate_match_request(registry, request).unwrap()
}

#[test]
fn registered_bindings_are_session_owned_fresh_and_thread_safe() {
    assert_send_sync::<SessionHandle>();
    assert_send_sync::<BindingHandle>();
    assert_send_sync::<FieldHandle>();
    assert_send_sync::<RoleHandle>();
    assert_send_sync::<PredicateHandle>();
    assert_send_sync::<OrderHandle>();
    assert_send_sync::<SelectionHandle>();
    assert_send_sync::<ShapeHandle>();
    assert_send_sync::<QueryHandle>();

    let session = SessionHandle::new(registry());
    let first = session.exact("person").unwrap();
    let second = session.exact("person").unwrap();
    let polymorphic = session.subtypes("person").unwrap();

    assert_ne!(first, second);
    assert_eq!(first.descriptor_id(), second.descriptor_id());
    assert_eq!(first.match_mode(), MatchMode::Exact);
    assert_eq!(polymorphic.match_mode(), MatchMode::Subtypes);
    assert_eq!(first.thing_kind(), ThingKind::Entity);
    assert_eq!(
        session.exact("employment").unwrap().thing_kind(),
        ThingKind::Relation
    );
    assert_match_code(session.exact("missing"), "unknown_descriptor");
}

#[test]
fn projected_function_handles_enforce_signature_domain_and_immutable_reuse() {
    assert_send_sync::<FunctionHandle>();
    assert_send_sync::<FunctionValueHandle>();
    assert_send_sync::<FunctionArgumentHandle>();
    assert_send_sync::<FunctionCallHandle>();

    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(Arc::clone(&registry));
    let qualifying = session
        .function(&FunctionId::new("qualifying-score").unwrap())
        .unwrap();
    let person_name = session
        .function(&FunctionId::new("person-name").unwrap())
        .unwrap();
    let identity_score = session
        .function(&FunctionId::new("identity-score").unwrap())
        .unwrap();
    let person = session.exact("person").unwrap();
    let person_subtypes = session.subtypes("person").unwrap();
    let employee = session.exact("employee").unwrap();
    let actor = session.subtypes("actor").unwrap();
    let minimum = ProjectedAttributeValue::try_new(
        &installed,
        TypeId::new(TypeKind::Attribute, "score").unwrap(),
        CanonicalValue::Long(40),
    )
    .unwrap();
    let minimum = session.function_value(&minimum).unwrap();
    let text = ProjectedAttributeValue::try_new(
        &installed,
        TypeId::new(TypeKind::Attribute, "name").unwrap(),
        CanonicalValue::String(CanonicalString::new("Alice").unwrap()),
    )
    .unwrap();
    let text = session.function_value(&text).unwrap();

    let projected_long = |projection: &InstalledRuntimeProjection| {
        ProjectedAttributeValue::try_new(
            projection,
            TypeId::new(TypeKind::Attribute, "score").unwrap(),
            CanonicalValue::Long(40),
        )
        .unwrap()
    };
    let changed_semantic = installed_function_projection_with_body(
        "match let $score = $minimum; return first $score;",
    );
    assert_function_value_package_mismatch(
        session.function_value(&projected_long(&changed_semantic)),
    );
    let foreign_target = installed_function_projection_with_evidence(
        QUALIFYING_SCORE_BODY,
        BindingTarget::TypeScript,
        &[],
    );
    assert_function_value_package_mismatch(
        session.function_value(&projected_long(&foreign_target)),
    );
    let resources =
        [
            CodeResourceDigest::from_bytes("test.function-resource", b"different emitter evidence")
                .unwrap(),
        ];
    let foreign_projection = installed_function_projection_with_evidence(
        QUALIFYING_SCORE_BODY,
        BindingTarget::Python,
        &resources,
    );
    assert_function_value_package_mismatch(
        session.function_value(&projected_long(&foreign_projection)),
    );

    for binding in [&person, &person_subtypes, &employee] {
        qualifying
            .call([binding.function_argument(), minimum.function_argument()])
            .expect("same/subtype declared binding domain is universally assignable");
    }
    assert_match_code(
        qualifying.call([actor.function_argument(), minimum.function_argument()]),
        "function_argument_type",
    );
    assert_match_code(
        qualifying.call([person.function_argument()]),
        "function_argument_arity",
    );

    let unattached_person = session.exact("person").unwrap();
    let unattached_inner = qualifying
        .call([
            unattached_person.function_argument(),
            minimum.function_argument(),
        ])
        .unwrap();
    let unattached_outer = identity_score
        .call([unattached_inner.function_argument()])
        .unwrap();
    let unattached_predicate = unattached_outer
        .compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
        .unwrap();
    assert_match_code(
        session
            .query(session.positional([person.one()]).unwrap())
            .unwrap()
            .where_predicate(unattached_predicate),
        "unattached_binding",
    );

    let joined_person = session.exact("person").unwrap();
    let joined_inner = qualifying
        .call([
            joined_person.function_argument(),
            minimum.function_argument(),
        ])
        .unwrap();
    let joined_outer = identity_score
        .call([joined_inner.function_argument()])
        .unwrap();
    let joined_request = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap()
        .add_hidden(joined_person)
        .unwrap()
        .where_predicate(
            joined_outer
                .compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
                .unwrap(),
        )
        .unwrap()
        .exists_by(&person)
        .unwrap();
    validate_match_request(&registry, joined_request)
        .expect("recursive model arguments contribute positive topology edges");

    let call = qualifying
        .call([person.function_argument(), minimum.function_argument()])
        .unwrap();
    let string_call = person_name.call([person.function_argument()]).unwrap();
    assert_match_code(
        call.compare_field(ComparisonOp::Equal, &person.field("name").unwrap()),
        "function_comparison_type",
    );
    assert_match_code(
        call.compare_value(ComparisonOp::Equal, &text),
        "function_comparison_type",
    );
    assert_match_code(
        call.compare_call(ComparisonOp::Equal, &string_call),
        "function_comparison_type",
    );
    assert_match_code(
        person
            .field("name")
            .unwrap()
            .compare_function(ComparisonOp::Equal, &call),
        "function_comparison_type",
    );
    assert_match_code(
        string_call.compare_field(ComparisonOp::Contains, &person.field("name").unwrap()),
        "function_comparison_operator_unsupported",
    );
    string_call
        .compare_value(ComparisonOp::Equal, &text)
        .expect("an operator rejection leaves both typed operands reusable");

    // Failed comparisons do not consume or mutate either reusable operand.
    let predicate = call
        .compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
        .unwrap();
    let request = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap()
        .where_predicate(predicate)
        .unwrap()
        .fetch_rows(
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
            RowCardinality::ExactlyOne,
        )
        .unwrap();
    let validated = validate_match_request(&registry, request).unwrap();
    assert!(
        validated
            .capabilities()
            .contains(Capability::SchemaFunctionCall)
    );

    let mut forged_without_call = validated.request().clone();
    let Some(MatchExpr::ScalarComparison { left, right, .. }) =
        forged_without_call.plan.predicate.as_mut()
    else {
        panic!("function handle lowers one scalar comparison")
    };
    *left = right.clone();
    let error = validate_match_request(&registry, forged_without_call)
        .expect_err("raw scalar operands cannot use the generated function-only request lane");
    assert_eq!(error.code().as_str(), "function_comparison_missing_call");

    assert_match_code(
        session.function(&FunctionId::new("find-people").unwrap()),
        "function_return_shape_unsupported",
    );
    assert_match_code(
        session.function(&FunctionId::new("missing").unwrap()),
        "unknown_projected_function",
    );
}

#[test]
fn projected_function_open_rejects_unrepresentable_arity_before_argument_materialization() {
    let installed = installed_wide_function_projection(MAX_BOOLEAN_TERMS + 1);
    let registry = Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(registry);
    let error = session
        .function(&FunctionId::new("wide-function").unwrap())
        .expect_err("a call wider than the request ceiling can never be represented");
    let OrmError::Match(error) = error else {
        panic!("function arity ceilings must remain structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
    assert_eq!(error.code().as_str(), "structural_limit_exceeded");
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Predicate]);
    assert_eq!(
        error.details().get("limit"),
        Some(&MatchErrorDetailValue::Text(
            "function_arguments".to_owned()
        ))
    );
}

#[test]
fn schema_function_requests_stale_when_only_the_function_body_changes() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let changed = installed_function_projection_with_body(
        "match let $score = $minimum; return first $score;",
    );
    let changed_registry = changed.match_registry().unwrap();

    assert_ne!(
        installed.projection().semantic_fingerprint(),
        changed.projection().semantic_fingerprint()
    );
    let error = validated
        .recheck_schema(&changed_registry)
        .expect_err("function body changes must stale an already validated request");
    assert_eq!(error.category(), MatchErrorCategory::StaleSchema);
    assert_eq!(error.code().as_str(), "stale_schema");
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Request]);
}

#[test]
fn schema_function_predicates_reject_or_and_not_without_consuming_the_call() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let minimum = ProjectedAttributeValue::try_new(
        &installed,
        TypeId::new(TypeKind::Attribute, "score").unwrap(),
        CanonicalValue::Long(40),
    )
    .unwrap();
    let minimum = session.function_value(&minimum).unwrap();
    let call = session
        .function(&FunctionId::new("qualifying-score").unwrap())
        .unwrap()
        .call([person.function_argument(), minimum.function_argument()])
        .unwrap();
    let function_predicate = || {
        call.compare_field(ComparisonOp::Equal, &person.field("score").unwrap())
            .unwrap()
    };
    let terminal = |predicate| {
        session
            .query(session.positional([person.one()]).unwrap())
            .unwrap()
            .where_predicate(predicate)
            .unwrap()
            .exists_by(&person)
            .unwrap()
    };

    for request in [
        terminal(function_predicate().not()),
        terminal(
            function_predicate()
                .or(&person.field("score").unwrap().presence(true))
                .unwrap(),
        ),
    ] {
        let error = validate_match_request(&registry, request)
            .expect_err("function calls are admitted only in root conjunction terms");
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(error.code().as_str(), "function_not_root");
        assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Predicate]);
    }

    validate_match_request(&registry, terminal(function_predicate()))
        .expect("the immutable call remains reusable after both rejected trees");
}

#[tokio::test]
async fn direct_schema_function_execution_uses_one_canonical_given_row() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: true,
            server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
        }),
        "test",
    );

    let result = database.execute_match(&registry, &validated).await.unwrap();
    assert!(matches!(
        result.result(),
        MatchResult::Exists { value: false, .. }
    ));
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 1);
    let given = events.given.lock().unwrap();
    let [(typeql, rows)] = given.as_slice() else {
        panic!("direct function execution must dispatch exactly one given statement")
    };
    assert!(typeql.starts_with("given $g0: integer;\nmatch\n"));
    assert!(typeql.contains("let $c0 = qualifying-score($b0, $g0)"));
    assert_eq!(rows.variables, ["g0"]);
    assert_eq!(rows.rows, [vec![GivenValue::Integer(40)]]);
    assert!(events.plain.lock().unwrap().is_empty());
}

#[tokio::test]
async fn direct_model_only_schema_function_keeps_empty_inputs_but_requires_full_capability() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = model_only_function_exists_request(&registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: true,
            server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
        }),
        "test",
    );

    let result = database.execute_match(&registry, &validated).await.unwrap();
    assert!(matches!(
        result.result(),
        MatchResult::Exists { value: false, .. }
    ));
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 1);
    assert!(events.given.lock().unwrap().is_empty());
    let plain = events.plain.lock().unwrap();
    let [typeql] = plain.as_slice() else {
        panic!("model-only function execution dispatches one ordinary typed statement")
    };
    assert!(typeql.starts_with("match\n"));
    assert!(typeql.contains("let $c0 = person-score($b0)"));
}

#[tokio::test]
async fn direct_model_only_schema_function_still_rejects_missing_given_capability_preopen() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = model_only_function_exists_request(&registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: false,
            server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
        }),
        "test",
    );

    let error = database
        .execute_match(&registry, &validated)
        .await
        .expect_err("the first function capability is given-bound even without input cells");
    let OrmError::Match(error) = error else {
        panic!("request-level capability rejection must stay structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
    assert_eq!(
        error.code().as_str(),
        "schema_function_given_rows_unsupported"
    );
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Operation]);
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 0);
    assert!(events.plain.lock().unwrap().is_empty());
    assert!(events.given.lock().unwrap().is_empty());
}

#[tokio::test]
async fn schema_function_given_capability_rejects_before_transaction_open() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: false,
            server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
        }),
        "test",
    );

    let error = database
        .execute_match(&registry, &validated)
        .await
        .expect_err("provider without given rows must fail pre-open");
    let OrmError::Match(error) = error else {
        panic!("given-row capability rejection must stay structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
    assert_eq!(
        error.code().as_str(),
        "schema_function_given_rows_unsupported"
    );
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Operation]);
    assert_eq!(
        error.details().get("capability"),
        Some(&MatchErrorDetailValue::Text(
            "query.input.given-rows".to_owned()
        ))
    );
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 0);
    assert!(events.given.lock().unwrap().is_empty());
}

#[tokio::test]
async fn schema_function_server_version_rejects_with_the_same_preopen_capability_error() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: true,
            server_version: type_bridge_core_lib::version::Version::new(3, 11, 5),
        }),
        "test",
    );

    let error = database
        .execute_match(&registry, &validated)
        .await
        .expect_err("a pre-given server must fail before transaction open");
    let OrmError::Match(error) = error else {
        panic!("server feature rejection must stay structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
    assert_eq!(
        error.code().as_str(),
        "schema_function_given_rows_unsupported"
    );
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Operation]);
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 0);
    assert!(events.given.lock().unwrap().is_empty());
}

#[tokio::test]
async fn borrowed_schema_function_given_capability_uses_the_same_structured_rejection() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: false,
            server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
        }),
        "test",
    );
    let context = database.transaction_context(TxType::Read).await.unwrap();

    let error = context
        .execute_match(&registry, &validated)
        .await
        .expect_err("borrowed provider without given rows must fail before a statement");
    let OrmError::Match(error) = error else {
        panic!("given-row capability rejection must stay structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
    assert_eq!(
        error.code().as_str(),
        "schema_function_given_rows_unsupported"
    );
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Operation]);
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 1);
    assert!(events.given.lock().unwrap().is_empty());
    context.close().await.unwrap();
}

#[tokio::test]
async fn borrowed_schema_function_server_version_uses_the_same_structured_rejection() {
    let installed = installed_function_projection();
    let registry = Arc::new(installed.match_registry().unwrap());
    let validated = function_exists_request(&installed, &registry);
    let events = Arc::new(FunctionProviderEvents::default());
    let database = Database::with_backend(
        Box::new(FunctionBackend {
            events: Arc::clone(&events),
            given: true,
            server_version: type_bridge_core_lib::version::Version::new(3, 11, 5),
        }),
        "test",
    );
    let context = database.transaction_context(TxType::Read).await.unwrap();

    let error = context
        .execute_match(&registry, &validated)
        .await
        .expect_err("a borrowed pre-given server must fail before a statement");
    let OrmError::Match(error) = error else {
        panic!("server feature rejection must stay structured")
    };
    assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
    assert_eq!(
        error.code().as_str(),
        "schema_function_given_rows_unsupported"
    );
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Operation]);
    assert_eq!(events.opens.load(AtomicOrdering::SeqCst), 1);
    assert!(events.given.lock().unwrap().is_empty());
    context.close().await.unwrap();
}

#[test]
fn owner_qualified_handles_reject_aliases_and_preserve_inherited_owners() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let employee = session.exact("employee").unwrap();
    let special = session.exact("special-association").unwrap();

    let inherited_name = employee.field_owned_by("party", "name").unwrap();
    assert_eq!(inherited_name.field_id().owner.as_str(), "entity:party");
    assert_match_code(
        person.field_owned_by("company", "name"),
        "cross_owner_field",
    );

    let inherited_role = special.role_owned_by("association", "participant").unwrap();
    assert_eq!(
        inherited_role.role_id().owner.as_str(),
        "relation:association"
    );
    assert_match_code(
        special.role_owned_by("collaboration", "participant"),
        "cross_owner_role",
    );

    let predicate = inherited_role.connects(&person).unwrap().and(
        &inherited_name
            .compare_field(ComparisonOp::Equal, &person.field("name").unwrap())
            .unwrap(),
    );
    let query = session
        .query(session.positional([employee.one(), special.one()]).unwrap())
        .unwrap()
        .add_hidden(person)
        .unwrap()
        .where_predicate(predicate.unwrap())
        .unwrap();
    let validated = query
        .validate_fetch_rows(
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
            RowCardinality::ExactlyOne,
        )
        .unwrap();
    let diagnostic = UnvalidatedMatchRequest::from_request(validated.request().clone()).unwrap();
    let encoded = String::from_utf8(diagnostic.to_canonical_bytes().unwrap()).unwrap();
    assert!(encoded.contains("entity:party"));
    assert!(encoded.contains("relation:association"));
}

#[test]
fn output_shape_arity_is_rejected_during_native_construction() {
    let session = SessionHandle::new(registry());
    assert_match_code(session.positional([]), "empty_output");
    assert_match_code(session.named::<_, String>([]), "empty_output");

    let slots = (0..=MAX_SELECTED_SLOTS)
        .map(|_| session.exact("person").unwrap().one())
        .collect::<Vec<_>>();
    assert_match_code(session.positional(slots), "selection_cap_exceeded");
}

#[test]
fn named_declaration_is_checked_against_native_selection_contracts() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let slots = || {
        vec![
            ("employee".to_owned(), person.one()),
            ("employers".to_owned(), company.collect()),
        ]
    };
    let declarations = || {
        vec![
            ("employee".to_owned(), "person".to_owned(), false),
            ("employers".to_owned(), "company".to_owned(), true),
        ]
    };

    session.named_checked(declarations(), slots()).unwrap();
    assert_match_code(
        session.named_checked(
            vec![("employee".to_owned(), "person".to_owned(), false)],
            slots(),
        ),
        "named_declaration_length_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("person".to_owned(), "person".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_name_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "company".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_descriptor_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "person".to_owned(), true),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_cardinality_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "missing".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "unknown_declared_descriptor",
    );
}

#[test]
fn named_shape_rejects_malformed_names_at_handle_construction() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    for malformed in [
        String::new(),
        "line\nbreak".to_owned(),
        "x".repeat(MAX_OUTPUT_NAME_BYTES + 1),
    ] {
        assert_match_code(
            session.named([(malformed.clone(), person.one())]),
            "invalid_output_name",
        );
        assert_match_code(
            session.named_checked(
                [(malformed.clone(), "person".to_owned(), false)],
                [(malformed, person.one())],
            ),
            "invalid_output_name",
        );
    }
}

#[test]
fn lowering_assigns_selected_then_hidden_contiguous_binding_ids() {
    let session = SessionHandle::new(registry());
    let selected_company = session.exact("company").unwrap();
    let selected_person = session.subtypes("person").unwrap();
    let hidden_relation = session.exact("employment").unwrap();
    let hidden_second_person = session.exact("person").unwrap();

    let shape = session
        .positional([selected_company.one(), selected_person.one()])
        .unwrap();
    let original = session.query(shape).unwrap();
    let with_relation = original.add_hidden(hidden_relation).unwrap();
    let complete = with_relation.add_hidden(hidden_second_person).unwrap();

    let original_request = original.count_by(&selected_company).unwrap();
    assert_eq!(original_request.plan.bindings.len(), 2);

    let request = complete.count_by(&selected_company).unwrap();
    assert_eq!(request.plan.bindings.len(), 4);
    assert_eq!(
        request
            .plan
            .bindings
            .iter()
            .map(|binding| (binding.id.get(), binding.descriptor.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (0, "entity:company"),
            (1, "entity:person"),
            (2, "relation:employment"),
            (3, "entity:person"),
        ]
    );
    assert_eq!(request.plan.bindings[1].match_mode, MatchMode::Subtypes);
    assert_eq!(request.plan.bindings[3].match_mode, MatchMode::Exact);
}

#[test]
fn immutable_transitions_lower_fields_roles_orders_shapes_and_cross_joins() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let employment = session.exact("employment").unwrap();

    let person_name = person.field("person-name").unwrap();
    let company_name = company.field("name").unwrap();
    let company_order = company_name.order(SortDirection::Ascending, MissingOrder::Last);
    let company_selection = company
        .collect()
        .distinct(true)
        .unwrap()
        .order_by(company_order.clone())
        .unwrap();
    let shape = session
        .named([("employee", person.one()), ("employers", company_selection)])
        .unwrap();
    let base = session.query(shape).unwrap();
    let attached = base.add_hidden(employment.clone()).unwrap();

    let employee_edge = employment
        .role("employee")
        .unwrap()
        .connects(&person)
        .unwrap();
    let employer_edge = employment
        .role("employer")
        .unwrap()
        .connects(&company)
        .unwrap();
    let named_alice =
        person_name.compare_value(ComparisonOp::Equal, AttributeValue::String("Alice".into()));
    let predicate = employee_edge
        .and(&employer_edge)
        .unwrap()
        .and(&named_alice.not())
        .unwrap();
    let filtered = attached.where_predicate(predicate).unwrap();
    let joined = filtered.allow_cross_join(&person, &company).unwrap();

    let request = joined
        .fetch_rows(
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 3,
                limit: 10,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap();

    assert_eq!(base.count_by(&person).unwrap().plan.bindings.len(), 2);
    assert!(attached.count_by(&person).unwrap().plan.predicate.is_none());
    assert_eq!(request.plan.allowed_cross_joins.len(), 1);
    assert_eq!(
        request.plan.allowed_cross_joins.iter().next().unwrap(),
        &BindingPair::new(BindingId::new(0), BindingId::new(1))
    );

    let MatchOperation::FetchRows {
        output,
        order,
        window,
        cardinality,
    } = &request.operation
    else {
        panic!("expected fetch rows")
    };
    assert_eq!(
        *window,
        Window {
            offset: 3,
            limit: 10
        }
    );
    assert_eq!(*cardinality, RowCardinality::BoundedMany);
    assert_eq!(order[0].field.binding, BindingId::new(0));
    assert_eq!(order[0].field.field.name, "name");
    let FetchShape::Named { slots } = output else {
        panic!("expected named output")
    };
    assert_eq!(slots[0].name, "employee");
    assert_eq!(slots[1].name, "employers");
    let FetchSlot::Collect {
        binding,
        distinct,
        order,
    } = &slots[1].slot
    else {
        panic!("expected collection slot")
    };
    assert_eq!(*binding, BindingId::new(1));
    assert!(*distinct);
    assert_eq!(order[0].field.binding, BindingId::new(1));

    let mut role_edges = Vec::new();
    collect_role_edge_ids(request.plan.predicate.as_ref().unwrap(), &mut role_edges);
    assert_eq!(role_edges, vec![0, 1]);
}

fn collect_role_edge_ids(expression: &MatchExpr, ids: &mut Vec<u16>) {
    match expression {
        MatchExpr::RoleEdge { id, .. } => ids.push(id.get()),
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for expression in expressions {
                collect_role_edge_ids(expression, ids);
            }
        }
        MatchExpr::Not { expression } => collect_role_edge_ids(expression, ids),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::ScalarComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::Reachable { .. } => {}
    }
}

#[test]
fn duplicate_cross_session_and_unattached_handles_are_rejected() {
    let registry = registry();
    let first = SessionHandle::new(Arc::clone(&registry));
    let second = SessionHandle::new(registry);
    let person = first.exact("person").unwrap();
    let company = first.exact("company").unwrap();
    let foreign_person = second.exact("person").unwrap();

    let duplicate_shape = first.positional([person.one(), person.one()]).unwrap();
    assert_match_code(first.query(duplicate_shape), "duplicate_selection");
    assert_match_code(
        first.positional([person.one(), foreign_person.one()]),
        "cross_session_handle",
    );

    let query = first
        .query(first.positional([person.one()]).unwrap())
        .unwrap();
    assert_match_code(
        query.add_hidden(foreign_person.clone()),
        "cross_session_handle",
    );
    assert_match_code(query.add_hidden(person.clone()), "duplicate_binding");
    assert_match_code(query.count_by(&company), "unattached_binding");
    assert_match_code(
        query.reduce_by_field(
            &person,
            &company.field("name").unwrap(),
            &[(Reduction::Count, None)],
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.reduce_by_field(
            &person,
            &foreign_person.field("name").unwrap(),
            &[(Reduction::Count, None)],
        ),
        "cross_session_handle",
    );
    assert_match_code(
        query.reduce_by_field(
            &person,
            &person.field("age").unwrap(),
            &[(Reduction::Sum, Some(&company.field("name").unwrap()))],
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.where_predicate(
            company
                .field("name")
                .unwrap()
                .compare_value(ComparisonOp::Equal, AttributeValue::String("Acme".into())),
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.where_predicate(
            company
                .field("name")
                .unwrap()
                .compare_value(ComparisonOp::Equal, AttributeValue::String("Acme".into()))
                .not(),
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.fetch_rows(
            &[company
                .field("name")
                .unwrap()
                .order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 0,
                limit: 5,
            },
            RowCardinality::BoundedMany,
        ),
        "unattached_binding",
    );
    assert_match_code(
        person
            .field("name")
            .unwrap()
            .compare_field(ComparisonOp::Equal, &foreign_person.field("name").unwrap()),
        "cross_session_handle",
    );
}

#[test]
fn bounded_reachability_is_session_owned_bounded_and_subtype_aware() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let other = SessionHandle::new(registry);
    let source = session.subtypes("node").unwrap();
    let target = session.subtypes("node").unwrap();
    let foreign = other.subtypes("node").unwrap();

    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            3,
            2,
        ),
        "reachable_bounds",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            65,
        ),
        "reachable_depth_limit",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &foreign,
            0,
            2,
        ),
        "cross_session_handle",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &session.exact("node").unwrap(),
            &target,
            0,
            2,
        ),
        "incompatible_reachable_endpoint",
    );

    let reachable = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            0,
            3,
        )
        .unwrap();
    let deep = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            64,
        )
        .unwrap();
    let deep_query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(deep.and(&deep).unwrap())
        .unwrap();
    assert_match_code(
        deep_query.validate_count_by(&source),
        "reachable_expansion_limit",
    );
    session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable.and(&reachable).unwrap())
        .unwrap()
        .validate_count_by(&source)
        .expect("nested root conjunction remains valid");
    let query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable)
        .unwrap();
    let validated = query.validate_count_by(&source).unwrap();
    let MatchExpr::Reachable {
        relation,
        role_from,
        role_to,
        source,
        target,
        min_depth,
        max_depth,
    } = validated.request().plan.predicate.as_ref().unwrap()
    else {
        panic!("expected bounded reachability")
    };
    assert_eq!(relation.as_str(), "relation:directed-edge");
    assert_eq!(role_from.name, "origin");
    assert_eq!(role_to.name, "destination");
    assert_eq!((*source, *target), (BindingId::new(0), BindingId::new(1)));
    assert_eq!((*min_depth, *max_depth), (0, 3));
    assert!(
        validated
            .capabilities()
            .contains(Capability::BoundedReachability)
    );
}

#[test]
fn bounded_reachability_is_rejected_under_disjunction_or_negation() {
    let session = SessionHandle::new(registry());
    let source = session.exact("leaf-node").unwrap();
    let target = session.exact("leaf-node").unwrap();
    let reachable = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            2,
        )
        .unwrap();

    let query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap();
    assert_match_code(
        query
            .where_predicate(reachable.or(&reachable).unwrap())
            .unwrap()
            .validate_count_by(&source),
        "reachable_not_root",
    );
    assert_match_code(
        query
            .where_predicate(reachable.not())
            .unwrap()
            .validate_count_by(&source),
        "reachable_not_root",
    );
}

#[test]
fn reachable_canonicalizes_an_inherited_role_to_the_exact_child_relation() {
    let session = SessionHandle::new(registry());
    let child_relation = session.exact("special-association").unwrap();
    let inherited = child_relation
        .role_owned_by("association", "participant")
        .expect("ancestor role reference remains effective on the child");
    let source = session.exact("person").unwrap();
    let target = session.exact("person").unwrap();

    let reachable = session
        .reachable(
            "special-association",
            &inherited.role_id().name,
            &inherited.role_id().name,
            &source,
            &target,
            1,
            2,
        )
        .expect("effective inherited role is accepted by name");
    let validated = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable)
        .unwrap()
        .validate_count_by(&source)
        .unwrap();
    let MatchExpr::Reachable {
        relation,
        role_from,
        role_to,
        ..
    } = validated.request().plan.predicate.as_ref().unwrap()
    else {
        panic!("expected bounded reachability");
    };
    assert_eq!(relation.as_str(), "relation:special-association");
    assert_eq!(
        role_from.owner.as_str(),
        "relation:special-association",
        "provider lowering receives the exact child relation-owned role",
    );
    assert_eq!(role_to.owner, role_from.owner);

    assert_match_code(
        session.reachable(
            "specialized-association",
            &inherited.role_id().name,
            &inherited.role_id().name,
            &source,
            &target,
            1,
            2,
        ),
        "unknown_role",
    );
}

#[test]
fn canonical_requests_ignore_process_local_allocation_history() {
    fn build(registry: Arc<DescriptorRegistry>, allocate_noise: bool) -> MatchRequest {
        let session = SessionHandle::new(registry);
        if allocate_noise {
            let _ = session.exact("person").unwrap();
            let _ = session.exact("employment").unwrap();
        }
        let person = session.exact("person").unwrap();
        let company = session.exact("company").unwrap();
        let employment = session.exact("employment").unwrap();
        if allocate_noise {
            let _ = session.exact("company").unwrap();
        }

        let shape = session.positional([person.one(), company.one()]).unwrap();
        let predicate = employment
            .role("employee")
            .unwrap()
            .connects(&person)
            .unwrap()
            .and(
                &employment
                    .role("employer")
                    .unwrap()
                    .connects(&company)
                    .unwrap(),
            )
            .unwrap();
        session
            .query(shape)
            .unwrap()
            .add_hidden(employment)
            .unwrap()
            .where_predicate(predicate)
            .unwrap()
            .page_by(
                &person,
                &[person
                    .field("age")
                    .unwrap()
                    .order(SortDirection::Descending, MissingOrder::Last)],
                Window {
                    offset: 5,
                    limit: 20,
                },
                true,
            )
            .unwrap()
    }

    let registry = registry();
    let first = build(Arc::clone(&registry), false);
    let second = build(registry, true);
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );

    let debug = format!(
        "{:?}",
        SessionHandle::new(Arc::new(DescriptorRegistry::new()))
    );
    assert!(debug.contains("SessionId(..)"));
    assert!(!debug.contains("73657373"));
    let serialized = serde_json::to_string(&first).unwrap();
    assert!(!serialized.contains("SessionId"));
    assert!(!serialized.contains("SessionBindingToken"));
}

#[test]
fn handle_requests_validate_and_survive_canonical_diagnostics() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let employment = session.exact("employment").unwrap();
    let person_name = person.field("name").unwrap();
    let company_name = company.field("name").unwrap();
    let connected = employment
        .role("employee")
        .unwrap()
        .connects(&person)
        .unwrap()
        .and(
            &employment
                .role("employer")
                .unwrap()
                .connects(&company)
                .unwrap(),
        )
        .unwrap();

    let positional = session
        .query(session.positional([person.one(), company.one()]).unwrap())
        .unwrap()
        .add_hidden(employment.clone())
        .unwrap()
        .where_predicate(connected.clone())
        .unwrap()
        .fetch_rows(
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 0,
                limit: 25,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap();
    let direct = validate_match_request(&registry, positional.clone()).unwrap();
    assert_eq!(direct.request(), &positional);
    assert_eq!(direct.stable_order().terms().len(), 2);

    let diagnostic = UnvalidatedMatchRequest::from_request(positional).unwrap();
    let bytes = diagnostic.to_canonical_bytes().unwrap();
    let decoded = UnvalidatedMatchRequest::from_canonical_bytes(&bytes).unwrap();
    let revalidated = decoded.validate(&registry).unwrap();
    assert_eq!(direct.request(), revalidated.request());
    assert_eq!(
        direct.schema_fingerprint(),
        revalidated.schema_fingerprint()
    );
    assert_eq!(direct.shape_id(), revalidated.shape_id());
    assert_eq!(direct.stable_order(), revalidated.stable_order());
    assert_eq!(direct.capabilities(), revalidated.capabilities());

    let collected_company = company
        .collect()
        .distinct(true)
        .unwrap()
        .order_by(company_name.order(SortDirection::Ascending, MissingOrder::Reject))
        .unwrap();
    let named_page = session
        .query(
            session
                .named([("person", person.one()), ("companies", collected_company)])
                .unwrap(),
        )
        .unwrap()
        .add_hidden(employment)
        .unwrap()
        .where_predicate(connected)
        .unwrap()
        .page_by(
            &person,
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 10,
                limit: 10,
            },
            true,
        )
        .unwrap();
    let validated_page = validate_match_request(&registry, named_page.clone()).unwrap();
    assert_eq!(validated_page.request(), &named_page);
    assert!(
        validated_page
            .capabilities()
            .contains(Capability::CollectDistinct)
    );
    assert!(
        validated_page
            .capabilities()
            .contains(Capability::DistinctRootSelection)
    );
}

#[test]
fn every_terminal_only_builds_an_unvalidated_request() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap();
    let order = [person
        .field("name")
        .unwrap()
        .order(SortDirection::Ascending, MissingOrder::Reject)];
    let window = Window {
        offset: 0,
        limit: 1,
    };

    assert!(matches!(
        query
            .fetch_rows(&order, window, RowCardinality::ExactlyOne)
            .unwrap()
            .operation,
        MatchOperation::FetchRows { .. }
    ));
    assert!(matches!(
        query
            .page_by(&person, &order, window, false)
            .unwrap()
            .operation,
        MatchOperation::PageBy { .. }
    ));
    assert!(matches!(
        query.count_by(&person).unwrap().operation,
        MatchOperation::CountBy { .. }
    ));
    assert!(matches!(
        query.exists_by(&person).unwrap().operation,
        MatchOperation::ExistsBy { .. }
    ));
    assert!(matches!(
        query
            .reduce_by(&person, None, &[(Reduction::Count, None)])
            .unwrap()
            .operation,
        MatchOperation::ReduceBy { .. }
    ));
    assert!(matches!(
        query
            .reduce_by_field(
                &person,
                &person.field("age").unwrap(),
                &[(Reduction::Count, None)],
            )
            .unwrap()
            .operation,
        MatchOperation::ReduceByField { .. }
    ));
}
