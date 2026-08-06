use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, CompleteReadProjection, CreateFieldProjection,
    CreateProjection, CreateRoleProjection, DeclarationProjection, DeclaredRoleProjection,
    DirectSubProjection, EmissionPlan, FieldTokenProjection, FunctionParameterProjection,
    FunctionProjection, FunctionReturnElementProjection, FunctionReturnProjection, ModelProjection,
    PlayingProjection, ProjectedModelForm, ProjectedModelUse, ProjectedMultiplicity,
    ProjectedTypeRef, ProjectionConfig, QueryTokenProjection, ReadFieldProjection,
    ReadRoleProjection, ReferenceReadProjection, RoleTokenProjection, RuntimeProjection,
    StructFieldProjection, StructProjection, TargetIdentifier,
};
use type_bridge_contract::schema::{
    AnnotationFactId, DocumentId, OwnsFactId, PlaysFactId, SchemaFactId, SubFactId,
};
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;
use type_bridge_contract::value::{Cardinality, ValueTypeTag};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::PythonEmitter;

fn multiplicity(min: u64, max: Option<u64>) -> ProjectedMultiplicity {
    ProjectedMultiplicity::from_cardinality(Cardinality::new(min, max).unwrap())
}

fn annotations() -> BTreeMap<AnnotationFactId, type_bridge_contract::projection::ProjectedAnnotation>
{
    BTreeMap::new()
}

fn direct_sub(subtype: &TypeId, supertype: &TypeId) -> DirectSubProjection {
    let id = SubFactId::new(subtype.clone(), supertype.clone()).unwrap();
    DirectSubProjection::new(id.clone(), SchemaFactId::Sub(id), annotations()).unwrap()
}

fn projected(
    source: &str,
    resources: &[CodeResourceDigest],
) -> type_bridge_contract::projection::RuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("python-emitter.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &PythonEmitter::new().generator_handlers(),
        resources,
    )
    .unwrap()
}

fn compound_projection(resources: &[CodeResourceDigest]) -> RuntimeProjection {
    let emitter = PythonEmitter::new();
    let identifier = TypeId::new(TypeKind::Attribute, "identifier").unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    let employee = RoleId::new("employment", "employee").unwrap();
    let plays_member = PlaysFactId::new(person.clone(), member.clone()).unwrap();
    let plays_employee = PlaysFactId::new(person.clone(), employee.clone()).unwrap();
    let one = multiplicity(1, Some(1));
    let many = multiplicity(0, None);
    let owns_identifier =
        OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();

    let identifier_model = ModelProjection::new(
        identifier.clone(),
        TargetIdentifier::python("Identifier").unwrap(),
        DeclarationProjection::new(
            None,
            Some(ValueTypeTag::String),
            false,
            true,
            annotations(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(true, vec![], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(None, vec![]).unwrap(),
        QueryTokenProjection::new(identifier.clone(), BTreeMap::new(), BTreeMap::new()).unwrap(),
    )
    .unwrap();

    let person_model = ModelProjection::new(
        person.clone(),
        TargetIdentifier::python("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            annotations(),
            vec![owns_identifier.clone()],
            BTreeMap::new(),
            BTreeSet::from([plays_member.clone(), plays_employee.clone()]),
        )
        .unwrap(),
        CreateProjection::new(
            true,
            vec![CreateFieldProjection::new(
                owns_identifier.clone(),
                ProjectedTypeRef::Model(ProjectedModelUse::new(
                    identifier.clone(),
                    ProjectedModelForm::Complete,
                )),
                one,
            )],
            BTreeMap::new(),
        )
        .unwrap(),
        CompleteReadProjection::new(
            vec![ReadFieldProjection::new(
                owns_identifier.clone(),
                ProjectedTypeRef::Model(ProjectedModelUse::new(
                    identifier.clone(),
                    ProjectedModelForm::Complete,
                )),
                one,
            )],
            BTreeMap::new(),
            vec![],
        )
        .unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::python("PersonRef").unwrap()),
            vec![owns_identifier.clone()],
        )
        .unwrap(),
        QueryTokenProjection::new(
            person.clone(),
            BTreeMap::from([(
                owns_identifier.clone(),
                FieldTokenProjection::new(
                    owns_identifier.clone(),
                    owns_identifier.clone(),
                    TargetIdentifier::python("identifier").unwrap(),
                    one,
                    true,
                    false,
                    annotations(),
                )
                .unwrap(),
            )]),
            BTreeMap::new(),
        )
        .unwrap(),
    )
    .unwrap();

    let membership_token = RoleTokenProjection::new(
        membership.clone(),
        member.clone(),
        TargetIdentifier::python("member").unwrap(),
        BTreeSet::from([person.clone()]),
        None,
        many,
        false,
        annotations(),
    )
    .unwrap();
    let membership_model = ModelProjection::new(
        membership.clone(),
        TargetIdentifier::python("Membership").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            annotations(),
            vec![],
            BTreeMap::from([(
                member.clone(),
                DeclaredRoleProjection::new(member.clone(), None),
            )]),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(
            true,
            vec![],
            BTreeMap::from([(
                member.clone(),
                CreateRoleProjection::new(
                    member.clone(),
                    BTreeSet::from([ProjectedModelUse::new(
                        person.clone(),
                        ProjectedModelForm::Complete,
                    )]),
                    many,
                )
                .unwrap(),
            )]),
        )
        .unwrap(),
        CompleteReadProjection::new(
            vec![],
            BTreeMap::from([(
                member.clone(),
                ReadRoleProjection::new(
                    member.clone(),
                    BTreeSet::from([ProjectedModelUse::new(
                        person.clone(),
                        ProjectedModelForm::Reference,
                    )]),
                    many,
                )
                .unwrap(),
            )]),
            vec![],
        )
        .unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::python("MembershipRef").unwrap()),
            vec![],
        )
        .unwrap(),
        QueryTokenProjection::new(
            membership.clone(),
            BTreeMap::new(),
            BTreeMap::from([(member.clone(), membership_token)]),
        )
        .unwrap(),
    )
    .unwrap();

    let employment_token = RoleTokenProjection::new(
        employment.clone(),
        employee.clone(),
        TargetIdentifier::python("employee").unwrap(),
        BTreeSet::from([person.clone()]),
        Some(member.clone()),
        one,
        false,
        annotations(),
    )
    .unwrap();
    let employment_model = ModelProjection::new(
        employment.clone(),
        TargetIdentifier::python("Employment").unwrap(),
        DeclarationProjection::new(
            Some(membership.clone()),
            None,
            false,
            true,
            annotations(),
            vec![],
            BTreeMap::from([(
                employee.clone(),
                DeclaredRoleProjection::new(employee.clone(), Some(member.clone())),
            )]),
            BTreeSet::new(),
        )
        .unwrap()
        .with_direct_sub(Some(direct_sub(&employment, &membership)))
        .unwrap(),
        CreateProjection::new(
            true,
            vec![],
            BTreeMap::from([(
                employee.clone(),
                CreateRoleProjection::new(
                    employee.clone(),
                    BTreeSet::from([ProjectedModelUse::new(
                        person.clone(),
                        ProjectedModelForm::Complete,
                    )]),
                    one,
                )
                .unwrap(),
            )]),
        )
        .unwrap(),
        CompleteReadProjection::new(
            vec![],
            BTreeMap::from([(
                employee.clone(),
                ReadRoleProjection::new(
                    employee.clone(),
                    BTreeSet::from([ProjectedModelUse::new(
                        person.clone(),
                        ProjectedModelForm::Reference,
                    )]),
                    one,
                )
                .unwrap(),
            )]),
            vec![membership.clone()],
        )
        .unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::python("EmploymentRef").unwrap()),
            vec![],
        )
        .unwrap(),
        QueryTokenProjection::new(
            employment.clone(),
            BTreeMap::new(),
            BTreeMap::from([(employee.clone(), employment_token)]),
        )
        .unwrap(),
    )
    .unwrap();

    let stats_id = StructId::new("player-stats").unwrap();
    let structure = StructProjection::new(
        stats_id.clone(),
        TargetIdentifier::python("PlayerStats").unwrap(),
        vec![
            StructFieldProjection::new(
                Label::new("wins").unwrap(),
                TargetIdentifier::python("wins").unwrap(),
                ValueTypeTag::Long,
                false,
            ),
            StructFieldProjection::new(
                Label::new("nickname").unwrap(),
                TargetIdentifier::python("nickname").unwrap(),
                ValueTypeTag::String,
                true,
            ),
        ],
    )
    .unwrap();
    let function_id = FunctionId::new("find-employment").unwrap();
    let function = FunctionProjection::new(
        function_id.clone(),
        TargetIdentifier::python("find_employment").unwrap(),
        vec![FunctionParameterProjection::new(
            Label::new("person").unwrap(),
            TargetIdentifier::python("person").unwrap(),
            ProjectedTypeRef::Model(ProjectedModelUse::new(
                person.clone(),
                ProjectedModelForm::Complete,
            )),
        )],
        FunctionReturnProjection::Stream(vec![FunctionReturnElementProjection::new(
            ProjectedTypeRef::Model(ProjectedModelUse::new(
                employment.clone(),
                ProjectedModelForm::Reference,
            )),
            false,
        )]),
    )
    .unwrap();
    let playing = BTreeMap::from([
        (
            plays_member.clone(),
            PlayingProjection::new(plays_member.clone(), member, many, annotations()).unwrap(),
        ),
        (
            plays_employee.clone(),
            PlayingProjection::new(plays_employee.clone(), employee, one, annotations()).unwrap(),
        ),
    ]);
    let models = BTreeMap::from([
        (identifier.clone(), identifier_model),
        (person.clone(), person_model),
        (membership.clone(), membership_model),
        (employment.clone(), employment_model),
    ]);
    let structs = BTreeMap::from([(stats_id.clone(), structure)]);
    let functions = BTreeMap::from([(function_id.clone(), function)]);
    let emission = EmissionPlan::new(
        vec![
            identifier.clone(),
            person.clone(),
            membership.clone(),
            employment.clone(),
        ],
        vec![
            BTreeSet::from([identifier]),
            BTreeSet::from([person]),
            BTreeSet::from([membership, employment]),
        ],
        vec![stats_id],
        vec![function_id],
    )
    .unwrap();
    RuntimeProjection::try_new(
        BindingTarget::Python,
        ProjectionConfig::python(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"compound projection",
        )
        .unwrap(),
        &emitter.generator_handlers(),
        resources,
        models,
        structs,
        functions,
        playing,
        emission,
    )
    .unwrap()
}

fn rebuild(
    projection: &RuntimeProjection,
    models: BTreeMap<TypeId, ModelProjection>,
    resources: &[CodeResourceDigest],
) -> RuntimeProjection {
    RuntimeProjection::try_new(
        projection.target(),
        projection.config().clone(),
        projection.semantic_fingerprint().clone(),
        projection.generator_handlers(),
        resources,
        models,
        projection.structs().clone(),
        projection.functions().clone(),
        projection.playing_facts().clone(),
        projection.emission().clone(),
    )
    .unwrap()
}

#[test]
fn emits_exact_deterministic_ten_file_compound_package() {
    let emitter = PythonEmitter::new();
    let projection = compound_projection(&emitter.code_resources().unwrap());
    let first = emitter.emit(&projection).unwrap();
    let second = emitter.emit(&projection).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first.files().keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "__init__.py",
            "__init__.pyi",
            "_models.py",
            "_models.pyi",
            "_query.py",
            "_query.pyi",
            "_runtime.py",
            "_runtime.pyi",
            "_schema.py",
            "py.typed",
        ]
    );
    let source = std::str::from_utf8(first.get("_models.py").unwrap()).unwrap();
    let stub = std::str::from_utf8(first.get("_models.pyi").unwrap()).unwrap();
    let schema = std::str::from_utf8(first.get("_schema.py").unwrap()).unwrap();
    assert!(
        source.find("class Employment(Membership):").unwrap()
            < source
                .find("# Projection-owned dependency-first SCC link phase.")
                .unwrap()
    );
    assert!(source.contains("class PlayerStats(_StructValue):"));
    assert!(source.contains(
        "find_employment: FunctionRef[[Person], Iterator[EmploymentRef]] = FunctionRef("
    ));
    assert!(source.contains("_install_runtime_projection("));
    assert!(source.contains("_initialize_attribute(self, value,"));
    assert!(stub.contains(
        "employee: _RoleDescriptor[Employment, Person, _BoundVar[Person] | _SubtypeBoundVar[Person], PersonRef, Person]"
    ));
    assert!(stub.contains("def __init__(self, *, employee: Person) -> None:"));
    assert!(
        stub.contains("find_employment: Final[FunctionRef[[Person], Iterator[EmploymentRef]]]")
    );
    assert!(schema.contains("SEMANTIC_SCHEMA_FINGERPRINT_JSON"));
    assert!(schema.contains("PROJECTION_FINGERPRINT_JSON"));
    assert!(schema.contains("PLAYING_FACTS = _MappingProxyType({"));
}

#[test]
fn emits_safely_escaped_type_and_direct_sub_documentation() {
    let emitter = PythonEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        r#"format: typebridge.schema/v2
entities:
  actor: {}
  person:
    doc: |-
      Type "doc".
      closing */ kept
    sub:
      type: actor
      doc: |-
        Edge 'doc' \ path
        closes */ safely
"#,
        &resources,
    );
    let package = emitter.emit(&projection).unwrap();
    let source = std::str::from_utf8(package.get("_models.py").unwrap()).unwrap();
    let stub = std::str::from_utf8(package.get("_models.pyi").unwrap()).unwrap();
    let documentation = "\"Type \\\"doc\\\".\\nclosing */ kept\\n\\nDirect subtype of `actor`:\\nEdge 'doc' \\\\ path\\ncloses */ safely\"";

    assert!(source.contains(&format!("class Person(Actor):\n    {documentation}")));
    assert!(source.contains(&format!("class PersonRef(ActorRef):\n    {documentation}")));
    assert!(stub.contains(&format!("class Person(Actor):\n    {documentation}")));
}

#[test]
fn rejects_mutated_resource_evidence() {
    let emitter = PythonEmitter::new();
    let mut resources = emitter.code_resources().unwrap();
    resources
        .retain(|resource| resource.id().as_str() != "typebridge.generator.python.runtime-source");
    resources.push(
        CodeResourceDigest::from_bytes(
            "typebridge.generator.python.runtime-source",
            b"mutated runtime",
        )
        .unwrap(),
    );
    resources.sort_by(|left, right| left.id().cmp(right.id()));
    let projection = compound_projection(&resources);
    assert_eq!(
        emitter.emit(&projection).unwrap_err().code().as_str(),
        "python_emitter_evidence_mismatch"
    );
}

#[test]
fn rejects_public_name_collisions_and_missing_parents() {
    let emitter = PythonEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = compound_projection(&resources);
    let employment_id = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let employment = &projection.models()[&employment_id];
    let collision = ModelProjection::new(
        employment.id().clone(),
        TargetIdentifier::python("Membership").unwrap(),
        employment.declaration().clone(),
        employment.create().clone(),
        employment.complete_read().clone(),
        employment.reference_read().clone(),
        employment.query_tokens().clone(),
    )
    .unwrap();
    let mut models = projection.models().clone();
    models.insert(employment_id.clone(), collision);
    let collision_projection = rebuild(&projection, models, &resources);
    assert_eq!(
        emitter
            .emit(&collision_projection)
            .unwrap_err()
            .code()
            .as_str(),
        "python_emitter_name_collision"
    );

    let ghost = TypeId::new(TypeKind::Relation, "ghost").unwrap();
    let declaration = DeclarationProjection::new(
        Some(ghost.clone()),
        employment.declaration().value_type(),
        employment.declaration().is_abstract(),
        employment.declaration().is_constructible(),
        employment.declaration().annotations().clone(),
        employment.declaration().direct_fields().to_vec(),
        employment.declaration().direct_roles().clone(),
        employment.declaration().direct_plays().clone(),
    )
    .unwrap()
    .with_direct_sub(Some(direct_sub(employment.id(), &ghost)))
    .unwrap();
    let missing_parent = ModelProjection::new(
        employment.id().clone(),
        employment.target_name().clone(),
        declaration,
        employment.create().clone(),
        employment.complete_read().clone(),
        employment.reference_read().clone(),
        employment.query_tokens().clone(),
    )
    .unwrap();
    let mut models = projection.models().clone();
    models.insert(employment_id, missing_parent);
    assert_eq!(
        RuntimeProjection::try_new(
            projection.target(),
            projection.config().clone(),
            projection.semantic_fingerprint().clone(),
            projection.generator_handlers(),
            &resources,
            models,
            projection.structs().clone(),
            projection.functions().clone(),
            projection.playing_facts().clone(),
            projection.emission().clone(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "invalid_projection_reference"
    );
}

#[test]
fn fixed_resources_are_distinct_and_content_addressed() {
    let resources = PythonEmitter::new().code_resources().unwrap();
    assert_eq!(resources.len(), 5);
    for (index, left) in resources.iter().enumerate() {
        for right in resources.iter().skip(index + 1) {
            assert_ne!(left.content_fingerprint(), right.content_fingerprint());
        }
    }
}
