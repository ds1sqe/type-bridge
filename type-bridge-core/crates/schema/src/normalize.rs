use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredSchema, DocText, FunctionBody, FunctionFact, FunctionParameter,
    FunctionReturnElement, FunctionReturnMode, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact,
    PlaysFactId, RegexPattern, RelatesFact, RelatesFactId, SchemaAnnotationValue, SchemaDiagnostic,
    SchemaDiagnostics, SchemaFact, SchemaFactId, SourceSpan, SourcedSchemaFact, StructFact,
    StructField, SubFact, SubFactId, TypeFact, TypeReference, ValueFact, ValueFactId,
};
use type_bridge_contract::temporal::{CanonicalDate, CanonicalDateTime, CanonicalDuration};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, Cardinality, DecimalValue, ValueTypeTag,
};

use crate::parse_provider_datetime_tz;
use crate::{FactAssembler, SchemaDocument, SchemaDocumentSet, YamlMapping, YamlNode, YamlScalar};

/// Exact discriminator for the first YAML Schema V2 document grammar.
pub const SCHEMA_V2_FORMAT: &str = "typebridge.schema/v2";

/// Normalize a lossless document set into provider-independent direct facts.
pub fn normalize_documents(
    documents: &SchemaDocumentSet,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    if documents.is_empty() {
        return Err(error(
            "empty_schema_document_set",
            "a schema document set must contain at least one document",
            None,
        ));
    }

    let mut normalizer = Normalizer::default();
    for (_, document) in documents.iter() {
        normalizer.normalize_document(document)?;
    }
    normalizer.finish()
}

#[derive(Default)]
struct Normalizer {
    facts: Vec<SourcedSchemaFact>,
    pending: Vec<PendingAnnotation>,
    pending_relates: Vec<PendingRelates>,
    pending_plays: Vec<YamlNode>,
    capabilities: CapabilitySet,
    capability_sources: BTreeMap<CapabilityId, SourceSpan>,
    type_labels: BTreeMap<String, (TypeId, SourceSpan)>,
}

impl Normalizer {
    fn normalize_document(&mut self, document: &SchemaDocument) -> Result<(), SchemaDiagnostics> {
        let root = document.root();
        check_keys(
            root,
            &[
                "format",
                "capabilities",
                "attributes",
                "entities",
                "relations",
                "plays",
                "functions",
                "structs",
                "extensions",
            ],
        )?;

        let format = required_entry(root, "format")?;
        let format = scalar(format.value())?;
        if format.value() != SCHEMA_V2_FORMAT {
            return Err(error(
                "unsupported_schema_document_format",
                format!("schema document format must be exactly `{SCHEMA_V2_FORMAT}`"),
                Some(format.span().clone()),
            ));
        }

        if let Some(entry) = entry(root, "capabilities") {
            self.normalize_capabilities(entry.value())?;
        }
        if let Some(entry) = entry(root, "extensions") {
            self.normalize_extensions(entry.value())?;
        }

        self.normalize_type_section(root, "attributes", TypeKind::Attribute)?;
        self.normalize_type_section(root, "entities", TypeKind::Entity)?;
        self.normalize_type_section(root, "relations", TypeKind::Relation)?;

        if let Some(entry) = entry(root, "plays") {
            self.pending_plays.push(entry.value().clone());
        }
        if let Some(entry) = entry(root, "functions") {
            self.normalize_functions(document, entry.value())?;
        }
        if let Some(entry) = entry(root, "structs") {
            self.normalize_structs(entry.value())?;
        }

        Ok(())
    }

    fn normalize_structs(&mut self, node: &YamlNode) -> Result<(), SchemaDiagnostics> {
        let declarations = mapping(node)?;

        for declaration in declarations.entries() {
            let id = contract(
                StructId::new(declaration.key().value()),
                declaration.key().span(),
            )?;
            let body = mapping(declaration.value())?;
            check_keys(body, &["fields"])?;

            let fields_node = required_entry(body, "fields")?.value();
            let fields_sequence = sequence(fields_node)?;
            let mut fields = Vec::with_capacity(fields_sequence.items().len());
            let mut field_sources = BTreeMap::<Label, SourceSpan>::new();

            for field_node in fields_sequence.items() {
                let field_body = mapping(field_node)?;
                check_keys(field_body, &["name", "type", "optional"])?;

                let name = scalar(required_entry(field_body, "name")?.value())?;
                let name_id = contract(Label::new(name.value()), name.span())?;

                if let Some(previous) = field_sources.insert(name_id.clone(), name.span().clone()) {
                    return Err(crate::yaml::diagnostic_with_related(
                        DiagnosticCategory::InvalidContract,
                        "duplicate_struct_field",
                        format!("struct field `{}` is declared more than once", name.value()),
                        name.span().clone(),
                        previous,
                        "first field declaration is here",
                    ));
                }

                let value_type =
                    parse_value_type(scalar(required_entry(field_body, "type")?.value())?)?;
                let optional = entry(field_body, "optional")
                    .map(|entry| strict_bool(entry.value()))
                    .transpose()?
                    .unwrap_or(false);

                fields.push(StructField::new(name_id, value_type, optional));
            }

            let fact = contract(StructFact::new(id, fields), body.span())?;
            self.push(SchemaFact::Struct(fact), body.span().clone());
        }

        Ok(())
    }

    fn normalize_capabilities(&mut self, node: &YamlNode) -> Result<(), SchemaDiagnostics> {
        let body = mapping(node)?;
        check_keys(body, &["required"])?;
        if let Some(required) = entry(body, "required") {
            let sequence = sequence(required.value())?;
            for item in sequence.items() {
                let capability = scalar(item)?;
                let id = contract(CapabilityId::new(capability.value()), capability.span())?;
                self.insert_capability(id, capability.span().clone())?;
            }
        }
        Ok(())
    }

    fn normalize_extensions(&mut self, node: &YamlNode) -> Result<(), SchemaDiagnostics> {
        let extensions = mapping(node)?;
        for extension in extensions.entries() {
            let id = contract(
                CapabilityId::new(extension.key().value()),
                extension.key().span(),
            )?;
            let body = mapping(extension.value())?;
            // V1 extension declarations are requirement-only. Payload-bearing
            // handlers are intentionally reserved for a future format version;
            // accepting and ignoring a payload would make it semantic dead data.
            check_keys(body, &["required"])?;
            let required = entry(body, "required")
                .map(|entry| strict_bool(entry.value()))
                .transpose()?
                .unwrap_or(false);
            if required {
                self.insert_capability(id, extension.key().span().clone())?;
            }
        }
        Ok(())
    }

    fn insert_capability(
        &mut self,
        id: CapabilityId,
        source: SourceSpan,
    ) -> Result<(), SchemaDiagnostics> {
        if let Some(previous) = self.capability_sources.get(&id) {
            return Err(crate::yaml::diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_required_capability",
                format!("required capability `{}` is duplicated", id.as_str()),
                source,
                previous.clone(),
                "first requirement is here",
            ));
        }
        self.capabilities.insert(id.clone());
        self.capability_sources.insert(id, source);
        Ok(())
    }

    fn normalize_type_section(
        &mut self,
        root: &YamlMapping,
        section_name: &str,
        kind: TypeKind,
    ) -> Result<(), SchemaDiagnostics> {
        let Some(section) = entry(root, section_name) else {
            return Ok(());
        };
        let section = mapping(section.value())?;

        for declaration in section.entries() {
            let id = contract(
                TypeId::new(kind, declaration.key().value()),
                declaration.key().span(),
            )?;
            self.register_type(id.clone(), declaration.key().span().clone())?;
            self.push(
                SchemaFact::Type(contract(
                    TypeFact::new(id.clone()),
                    declaration.key().span(),
                )?),
                declaration.key().span().clone(),
            );

            let body = mapping(declaration.value())?;
            let allowed = match kind {
                TypeKind::Attribute => {
                    &["sub", "value", "abstract", "independent", "doc", "meta"][..]
                }
                TypeKind::Entity => &["sub", "owns", "abstract", "doc", "meta"][..],
                TypeKind::Relation => &["sub", "owns", "relates", "abstract", "doc", "meta"][..],
                TypeKind::Struct => &[][..],
            };
            check_keys(body, allowed)?;

            self.queue_presence(
                body,
                "abstract",
                AnnotationSubjectId::Type(id.clone()),
                AnnotationKindId::Abstract,
            )?;
            if kind == TypeKind::Attribute {
                self.queue_presence(
                    body,
                    "independent",
                    AnnotationSubjectId::Type(id.clone()),
                    AnnotationKindId::Independent,
                )?;
            }
            self.queue_doc_meta(body, AnnotationSubjectId::Type(id.clone()))?;

            if let Some(sub) = entry(body, "sub") {
                let (parent, annotations) = match sub.value() {
                    YamlNode::Scalar(parent) => (parent, None),
                    YamlNode::Mapping(expanded) => {
                        check_keys(expanded, &["type", "doc", "meta"])?;
                        let parent = scalar(required_entry(expanded, "type")?.value())?;
                        (parent, Some(expanded))
                    }
                    YamlNode::Sequence(sequence) => {
                        return Err(error(
                            "invalid_schema_sub_shape",
                            "sub must be a scalar or mapping",
                            Some(sequence.span().clone()),
                        ));
                    }
                };
                let parent_id = contract(TypeId::new(kind, parent.value()), parent.span())?;
                let sub_id = contract(SubFactId::new(id.clone(), parent_id), parent.span())?;
                self.push(
                    SchemaFact::Sub(SubFact::new(sub_id.clone())),
                    parent.span().clone(),
                );
                if let Some(annotations) = annotations {
                    self.queue_doc_meta(annotations, AnnotationSubjectId::Sub(sub_id))?;
                }
            }

            if kind == TypeKind::Attribute
                && let Some(value) = entry(body, "value")
            {
                self.normalize_value(&id, value.value())?;
            }
            if matches!(kind, TypeKind::Entity | TypeKind::Relation)
                && let Some(owns) = entry(body, "owns")
            {
                self.normalize_owns(&id, owns.value())?;
            }
            if kind == TypeKind::Relation
                && let Some(relates) = entry(body, "relates")
            {
                self.normalize_relates(&id, relates.value())?;
            }
        }
        Ok(())
    }

    fn register_type(&mut self, id: TypeId, source: SourceSpan) -> Result<(), SchemaDiagnostics> {
        let label = id.label().as_str().to_owned();
        if let Some((_, previous)) = self.type_labels.get(&label) {
            return Err(crate::yaml::diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_schema_type_label",
                format!("schema type label `{label}` is duplicated"),
                source,
                previous.clone(),
                "first type label is here",
            ));
        }
        self.type_labels.insert(label, (id, source));
        Ok(())
    }

    fn normalize_value(
        &mut self,
        attribute: &TypeId,
        node: &YamlNode,
    ) -> Result<(), SchemaDiagnostics> {
        let (value_type, source, annotations) = match node {
            YamlNode::Scalar(value) => (parse_value_type(value)?, value.span().clone(), None),
            YamlNode::Mapping(body) => {
                check_keys(body, &["type", "regex", "range", "values"])?;
                let value = required_entry(body, "type")?;
                let value = scalar(value.value())?;
                (parse_value_type(value)?, value.span().clone(), Some(body))
            }
            YamlNode::Sequence(sequence) => {
                return Err(error(
                    "invalid_schema_value_shape",
                    "attribute value must be a scalar or mapping",
                    Some(sequence.span().clone()),
                ));
            }
        };

        let attribute_id = contract(AttributeId::new(attribute.label().as_str()), &source)?;
        let value_id = ValueFactId::new(attribute_id);
        self.push(
            SchemaFact::Value(ValueFact::new(value_id.clone(), value_type)),
            source,
        );

        if let Some(body) = annotations {
            self.queue_value_annotations(body, AnnotationSubjectId::Value(value_id))?;
        }
        Ok(())
    }

    fn normalize_owns(&mut self, owner: &TypeId, node: &YamlNode) -> Result<(), SchemaDiagnostics> {
        for named in named_bodies(node)? {
            let attribute = contract(AttributeId::new(named.name.value()), named.name.span())?;
            let id = contract(OwnsFactId::new(owner.clone(), attribute), &named.source)?;
            self.push(
                SchemaFact::Owns(OwnsFact::new(id.clone())),
                named.source.clone(),
            );

            if let Some(body) = &named.body {
                check_keys(
                    body,
                    &[
                        "key", "unique", "card", "regex", "range", "values", "doc", "meta",
                    ],
                )?;
                let subject = AnnotationSubjectId::Owns(id);
                self.queue_presence(body, "key", subject.clone(), AnnotationKindId::Key)?;
                self.queue_presence(body, "unique", subject.clone(), AnnotationKindId::Unique)?;
                self.queue_if_present(
                    body,
                    "card",
                    subject.clone(),
                    AnnotationKindId::Card,
                    PendingInput::Card,
                )?;
                self.queue_value_annotations(body, subject.clone())?;
                self.queue_doc_meta(body, subject)?;
            }
        }
        Ok(())
    }

    fn normalize_relates(
        &mut self,
        relation: &TypeId,
        node: &YamlNode,
    ) -> Result<(), SchemaDiagnostics> {
        for named in named_bodies(node)? {
            let role = contract(
                RoleId::new(relation.label().as_str(), named.name.value()),
                named.name.span(),
            )?;
            let specializes = if let Some(body) = &named.body {
                if let Some(specializes) = entry(body, "as") {
                    let specializes = scalar(specializes.value())?;
                    Some((specializes.value().to_owned(), specializes.span().clone()))
                } else {
                    None
                }
            } else {
                None
            };
            let id = contract(RelatesFactId::new(relation.clone(), role), &named.source)?;
            self.pending_relates.push(PendingRelates {
                id: id.clone(),
                specializes,
                source: named.source.clone(),
            });

            if let Some(body) = &named.body {
                check_keys(body, &["as", "abstract", "card", "doc", "meta"])?;
                let subject = AnnotationSubjectId::Relates(id);
                self.queue_presence(
                    body,
                    "abstract",
                    subject.clone(),
                    AnnotationKindId::Abstract,
                )?;
                self.queue_if_present(
                    body,
                    "card",
                    subject.clone(),
                    AnnotationKindId::Card,
                    PendingInput::Card,
                )?;
                self.queue_doc_meta(body, subject)?;
            }
        }
        Ok(())
    }

    fn normalize_plays(&mut self, node: &YamlNode) -> Result<(), SchemaDiagnostics> {
        let players = mapping(node)?;
        for player_entry in players.entries() {
            let Some((player, _)) = self.type_labels.get(player_entry.key().value()).cloned()
            else {
                return Err(error(
                    "unknown_schema_player",
                    "root plays declaration references an unknown player type",
                    Some(player_entry.key().span().clone()),
                ));
            };
            if !matches!(player.kind(), TypeKind::Entity | TypeKind::Relation) {
                return Err(error(
                    "invalid_schema_player_kind",
                    "only entity and relation types may play roles",
                    Some(player_entry.key().span().clone()),
                ));
            }
            let relations = mapping(player_entry.value())?;
            for relation_entry in relations.entries() {
                let Some((relation, _)) =
                    self.type_labels.get(relation_entry.key().value()).cloned()
                else {
                    return Err(error(
                        "unknown_schema_relation",
                        "root plays declaration references an unknown relation type",
                        Some(relation_entry.key().span().clone()),
                    ));
                };
                if relation.kind() != TypeKind::Relation {
                    return Err(error(
                        "invalid_schema_relation_kind",
                        "root plays relation key must name a relation type",
                        Some(relation_entry.key().span().clone()),
                    ));
                }
                for named in named_bodies(relation_entry.value())? {
                    let role = contract(
                        RoleId::new(relation.label().as_str(), named.name.value()),
                        named.name.span(),
                    )?;
                    let id = contract(PlaysFactId::new(player.clone(), role), &named.source)?;
                    self.push(
                        SchemaFact::Plays(PlaysFact::new(id.clone())),
                        named.source.clone(),
                    );
                    if let Some(body) = &named.body {
                        check_keys(body, &["card", "doc", "meta"])?;
                        let subject = AnnotationSubjectId::Plays(id);
                        self.queue_if_present(
                            body,
                            "card",
                            subject.clone(),
                            AnnotationKindId::Card,
                            PendingInput::Card,
                        )?;
                        self.queue_doc_meta(body, subject)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn normalize_functions(
        &mut self,
        _document: &SchemaDocument,
        node: &YamlNode,
    ) -> Result<(), SchemaDiagnostics> {
        let functions = mapping(node)?;
        for declaration in functions.entries() {
            let body = mapping(declaration.value())?;
            check_keys(body, &["parameters", "returns", "body", "doc", "meta"])?;
            let typeql_body = mapping(required_entry(body, "body")?.value())?;
            check_keys(typeql_body, &["typeql"])?;
            let body_text = scalar(required_entry(typeql_body, "typeql")?.value())?;

            let mut parameters_out = Vec::new();
            let mut parameter_sources = BTreeMap::<Label, SourceSpan>::new();
            if let Some(parameters) = entry(body, "parameters") {
                for parameter_node in sequence(parameters.value())?.items() {
                    let parameter = mapping(parameter_node)?;
                    check_keys(parameter, &["name", "type"])?;
                    let name = scalar(required_entry(parameter, "name")?.value())?;
                    let name_id = contract(Label::new(name.value()), name.span())?;
                    if let Some(previous) =
                        parameter_sources.insert(name_id.clone(), name.span().clone())
                    {
                        return Err(crate::diagnostic::diagnostic_with_related(
                            DiagnosticCategory::InvalidContract,
                            "duplicate_function_parameter",
                            format!("function parameter `{}` is duplicated", name.value()),
                            name.span().clone(),
                            previous,
                            "first parameter declaration is here",
                        ));
                    }
                    let type_token = scalar(required_entry(parameter, "type")?.value())?;
                    let type_ref = contract(
                        TypeReference::from_token(type_token.value()),
                        type_token.span(),
                    )?;
                    parameters_out.push(FunctionParameter::new(name_id, type_ref));
                }
            }

            let returns = mapping(required_entry(body, "returns")?.value())?;
            check_keys(returns, &["stream"])?;
            let stream = sequence(required_entry(returns, "stream")?.value())?;
            let mut return_elements = Vec::with_capacity(stream.items().len());
            for element in stream.items() {
                let type_token = scalar(element)?;
                let type_ref = contract(
                    TypeReference::from_token(type_token.value()),
                    type_token.span(),
                )?;
                return_elements.push(FunctionReturnElement::new(type_ref, false));
            }

            let id = contract(
                FunctionId::new(declaration.key().value()),
                declaration.key().span(),
            )?;
            let returns = contract(FunctionReturnMode::stream(return_elements), returns.span())?;
            let signature = contract(FunctionSignature::new(parameters_out, returns), body.span())?;
            let function_body = contract(FunctionBody::new(body_text.value()), body_text.span())?;
            self.push(
                SchemaFact::Function(FunctionFact::new(id.clone(), signature, function_body)),
                body.span().clone(),
            );
            self.queue_doc_meta(body, AnnotationSubjectId::Function(id))?;
        }
        Ok(())
    }

    fn queue_presence(
        &mut self,
        body: &YamlMapping,
        key: &str,
        subject: AnnotationSubjectId,
        kind: AnnotationKindId,
    ) -> Result<(), SchemaDiagnostics> {
        let Some(entry) = entry(body, key) else {
            return Ok(());
        };
        if !strict_bool(entry.value())? {
            return Err(error(
                "false_presence_annotation",
                "presence annotations must be omitted rather than set to false",
                Some(entry.value().span().clone()),
            ));
        }
        self.pending.push(PendingAnnotation {
            subject,
            kind,
            input: PendingInput::Presence,
            source: entry.value().span().clone(),
        });
        Ok(())
    }

    fn queue_if_present(
        &mut self,
        body: &YamlMapping,
        key: &str,
        subject: AnnotationSubjectId,
        kind: AnnotationKindId,
        input: fn(YamlNode) -> PendingInput,
    ) -> Result<(), SchemaDiagnostics> {
        if let Some(entry) = entry(body, key) {
            self.pending.push(PendingAnnotation {
                subject,
                kind,
                input: input(entry.value().clone()),
                source: entry.value().span().clone(),
            });
        }
        Ok(())
    }

    fn queue_value_annotations(
        &mut self,
        body: &YamlMapping,
        subject: AnnotationSubjectId,
    ) -> Result<(), SchemaDiagnostics> {
        self.queue_if_present(
            body,
            "regex",
            subject.clone(),
            AnnotationKindId::Regex,
            PendingInput::Regex,
        )?;
        self.queue_if_present(
            body,
            "range",
            subject.clone(),
            AnnotationKindId::Range,
            PendingInput::Range,
        )?;
        self.queue_if_present(
            body,
            "values",
            subject,
            AnnotationKindId::Values,
            PendingInput::Values,
        )
    }

    fn queue_doc_meta(
        &mut self,
        body: &YamlMapping,
        subject: AnnotationSubjectId,
    ) -> Result<(), SchemaDiagnostics> {
        self.queue_if_present(
            body,
            "doc",
            subject.clone(),
            AnnotationKindId::Doc,
            PendingInput::Doc,
        )?;
        if let Some(meta) = entry(body, "meta") {
            let meta = mapping(meta.value())?;
            for item in meta.entries() {
                let kind = contract(
                    AnnotationKindId::meta(item.key().value()),
                    item.key().span(),
                )?;
                self.pending.push(PendingAnnotation {
                    subject: subject.clone(),
                    kind,
                    input: PendingInput::Meta(item.value().clone()),
                    source: item.value().span().clone(),
                });
            }
        }
        Ok(())
    }

    fn push(&mut self, fact: SchemaFact, source: SourceSpan) {
        self.facts.push(SourcedSchemaFact::new(fact, source));
    }

    fn finish(mut self) -> Result<DeclaredSchema, SchemaDiagnostics> {
        self.materialize_relates()?;
        for plays in std::mem::take(&mut self.pending_plays) {
            self.normalize_plays(&plays)?;
        }
        let mut assembler = FactAssembler::new(FormatVersion::V1);
        for (capability, source) in &self.capability_sources {
            assembler.require_capability(capability.clone(), source.clone())?;
        }
        for sourced in &self.facts {
            match sourced.fact() {
                SchemaFact::Struct(fact) => {
                    assembler.insert_struct(fact.clone(), sourced.source().clone())?;
                }
                fact => {
                    assembler.insert_fact(fact.clone(), sourced.source().clone())?;
                }
            }
        }
        let structural = assembler.finish()?;
        for pending in self.pending {
            self.facts.push(pending.resolve(&structural)?);
        }
        DeclaredSchema::from_facts(FormatVersion::V1, self.capabilities, self.facts)
    }

    fn materialize_relates(&mut self) -> Result<(), SchemaDiagnostics> {
        let declarations = std::mem::take(&mut self.pending_relates);
        for declaration in &declarations {
            let specializes = declaration
                .specializes
                .as_ref()
                .map(|(label, source)| {
                    self.resolve_inherited_role(
                        declaration.id.relation(),
                        label,
                        source,
                        &declarations,
                    )
                })
                .transpose()?;
            self.push(
                SchemaFact::Relates(contract(
                    RelatesFact::new(declaration.id.clone(), specializes),
                    &declaration.source,
                )?),
                declaration.source.clone(),
            );
        }
        Ok(())
    }

    fn resolve_inherited_role(
        &self,
        relation: &TypeId,
        role_label: &str,
        source: &SourceSpan,
        declarations: &[PendingRelates],
    ) -> Result<RoleId, SchemaDiagnostics> {
        let mut current = relation.clone();
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.clone()) {
                return Err(error(
                    "schema_inheritance_cycle",
                    "relation inheritance contains a cycle",
                    Some(source.clone()),
                ));
            }
            let Some(parent) = self.direct_parent(&current) else {
                return Err(error(
                    "invalid_role_specialization",
                    "specialized role is not declared by any ancestor relation",
                    Some(source.clone()),
                ));
            };
            if let Some(role) = declarations.iter().find_map(|candidate| {
                (candidate.id.relation() == &parent
                    && candidate.id.role().label().as_str() == role_label)
                    .then(|| candidate.id.role().clone())
            }) {
                return Ok(role);
            }
            current = parent;
        }
    }

    fn direct_parent(&self, subtype: &TypeId) -> Option<TypeId> {
        self.facts.iter().find_map(|sourced| {
            let SchemaFact::Sub(sub) = sourced.fact() else {
                return None;
            };
            (sub.id().subtype() == subtype).then(|| sub.id().supertype().clone())
        })
    }
}

struct PendingRelates {
    id: RelatesFactId,
    specializes: Option<(String, SourceSpan)>,
    source: SourceSpan,
}

struct PendingAnnotation {
    subject: AnnotationSubjectId,
    kind: AnnotationKindId,
    input: PendingInput,
    source: SourceSpan,
}

enum PendingInput {
    Presence,
    Card(YamlNode),
    Regex(YamlNode),
    Range(YamlNode),
    Values(YamlNode),
    Doc(YamlNode),
    Meta(YamlNode),
}

impl PendingAnnotation {
    fn resolve(self, schema: &DeclaredSchema) -> Result<SourcedSchemaFact, SchemaDiagnostics> {
        let value = match self.input {
            PendingInput::Presence => SchemaAnnotationValue::Presence,
            PendingInput::Card(node) => {
                SchemaAnnotationValue::Cardinality(parse_cardinality(&node)?)
            }
            PendingInput::Regex(node) => SchemaAnnotationValue::Regex(contract(
                RegexPattern::new(scalar(&node)?.value()),
                node.span(),
            )?),
            PendingInput::Range(node) => {
                let value_type = subject_value_type(schema, &self.subject, &self.source)?;
                SchemaAnnotationValue::Range(parse_range(&node, value_type)?)
            }
            PendingInput::Values(node) => {
                let value_type = subject_value_type(schema, &self.subject, &self.source)?;
                SchemaAnnotationValue::Values(parse_values(&node, value_type)?)
            }
            PendingInput::Doc(node) => SchemaAnnotationValue::Doc(contract(
                DocText::new(scalar(&node)?.value()),
                node.span(),
            )?),
            PendingInput::Meta(node) => {
                let scalar = scalar(&node)?;
                SchemaAnnotationValue::Meta(CanonicalValue::String(contract(
                    CanonicalString::new(scalar.value()),
                    scalar.span(),
                )?))
            }
        };
        let fact = contract(
            AnnotationFact::new(AnnotationFactId::new(self.subject, self.kind), value),
            &self.source,
        )?;
        Ok(SourcedSchemaFact::new(
            SchemaFact::Annotation(fact),
            self.source,
        ))
    }
}

#[derive(Clone)]
struct NamedBody {
    name: YamlScalar,
    body: Option<YamlMapping>,
    source: SourceSpan,
}

fn named_bodies(node: &YamlNode) -> Result<Vec<NamedBody>, SchemaDiagnostics> {
    match node {
        YamlNode::Mapping(mapping) => mapping.entries().iter().map(named_mapping_entry).collect(),
        YamlNode::Sequence(sequence) => sequence
            .items()
            .iter()
            .map(|item| match item {
                YamlNode::Scalar(name) => Ok(NamedBody {
                    name: name.clone(),
                    body: None,
                    source: name.span().clone(),
                }),
                YamlNode::Mapping(mapping) if mapping.entries().len() == 1 => {
                    named_mapping_entry(&mapping.entries()[0])
                }
                other => Err(error(
                    "invalid_named_schema_fact",
                    "named facts must be scalars or single-entry mappings",
                    Some(other.span().clone()),
                )),
            })
            .collect(),
        YamlNode::Scalar(value) => Err(error(
            "invalid_named_schema_fact_collection",
            "named facts must be a sequence or mapping",
            Some(value.span().clone()),
        )),
    }
}

fn named_mapping_entry(entry: &crate::YamlMappingEntry) -> Result<NamedBody, SchemaDiagnostics> {
    let (body, source) = match entry.value() {
        YamlNode::Mapping(body) => (Some(body.clone()), body.span().clone()),
        other => {
            return Err(error(
                "invalid_named_schema_fact_body",
                "expanded named fact bodies must be mappings",
                Some(other.span().clone()),
            ));
        }
    };
    Ok(NamedBody {
        name: entry.key().clone(),
        body,
        source,
    })
}

fn subject_value_type(
    schema: &DeclaredSchema,
    subject: &AnnotationSubjectId,
    source: &SourceSpan,
) -> Result<ValueTypeTag, SchemaDiagnostics> {
    let mut attribute = match subject {
        AnnotationSubjectId::Value(id) => id.attribute().clone(),
        AnnotationSubjectId::Owns(id) => id.attribute().clone(),
        _ => {
            return Err(error(
                "annotation_value_domain_unavailable",
                "value annotation subject has no attribute domain",
                Some(source.clone()),
            ));
        }
    };
    let mut visited = BTreeSet::new();
    loop {
        let type_id = contract(
            TypeId::new(TypeKind::Attribute, attribute.label().as_str()),
            source,
        )?;
        if !visited.insert(type_id.clone()) {
            return Err(error(
                "schema_value_inheritance_cycle",
                "attribute value inheritance contains a cycle",
                Some(source.clone()),
            ));
        }
        let value_id = ValueFactId::new(attribute.clone());
        if let Some(SchemaFact::Value(value)) = schema.fact(&SchemaFactId::Value(value_id)) {
            return Ok(value.value_type());
        }
        let Some(parent) = schema.facts().find_map(|fact| {
            let SchemaFact::Sub(sub) = fact else {
                return None;
            };
            (sub.id().subtype() == &type_id && sub.id().supertype().kind() == TypeKind::Attribute)
                .then(|| sub.id().supertype().clone())
        }) else {
            return Err(error(
                "schema_value_domain_missing",
                "attribute has no direct or inherited value domain",
                Some(source.clone()),
            ));
        };
        attribute = contract(AttributeId::new(parent.label().as_str()), source)?;
    }
}

fn parse_cardinality(node: &YamlNode) -> Result<Cardinality, SchemaDiagnostics> {
    match node {
        YamlNode::Scalar(value) => {
            let exact = canonical_u64(value)?;
            contract(Cardinality::new(exact, Some(exact)), value.span())
        }
        YamlNode::Mapping(body) => {
            check_keys(body, &["min", "max"])?;
            let min = canonical_u64(scalar(required_entry(body, "min")?.value())?)?;
            let max = entry(body, "max")
                .map(|entry| canonical_u64(scalar(entry.value())?))
                .transpose()?;
            contract(Cardinality::new(min, max), body.span())
        }
        YamlNode::Sequence(sequence) => Err(error(
            "invalid_cardinality_shape",
            "cardinality must be an integer or a min/max mapping",
            Some(sequence.span().clone()),
        )),
    }
}

fn parse_values(
    node: &YamlNode,
    value_type: ValueTypeTag,
) -> Result<CanonicalValueSet, SchemaDiagnostics> {
    let values = sequence(node)?;
    let canonical = values
        .items()
        .iter()
        .map(|item| canonical_value(value_type, scalar(item)?))
        .collect::<Result<Vec<_>, _>>()?;
    contract(CanonicalValueSet::new(canonical), values.span())
}

fn parse_range(
    node: &YamlNode,
    value_type: ValueTypeTag,
) -> Result<CanonicalValueRange, SchemaDiagnostics> {
    let body = mapping(node)?;
    check_keys(body, &["min", "max"])?;
    let lower = entry(body, "min")
        .map(|entry| canonical_value(value_type, scalar(entry.value())?))
        .transpose()?;
    let upper = entry(body, "max")
        .map(|entry| canonical_value(value_type, scalar(entry.value())?))
        .transpose()?;
    contract(CanonicalValueRange::new(lower, upper), body.span())
}

fn canonical_value(
    value_type: ValueTypeTag,
    scalar: &YamlScalar,
) -> Result<CanonicalValue, SchemaDiagnostics> {
    let spelling = scalar.value();
    match value_type {
        ValueTypeTag::String => Ok(CanonicalValue::String(contract(
            CanonicalString::new(spelling),
            scalar.span(),
        )?)),
        ValueTypeTag::Long => {
            let value = spelling.parse::<i64>().map_err(|_| {
                error(
                    "invalid_integer_value",
                    "integer annotation value is not an i64",
                    Some(scalar.span().clone()),
                )
            })?;
            if value.to_string() != spelling {
                return Err(error(
                    "non_canonical_integer_value",
                    "integer annotation value is not canonically spelled",
                    Some(scalar.span().clone()),
                ));
            }
            Ok(CanonicalValue::Long(value))
        }
        ValueTypeTag::Double => {
            let value = spelling.parse::<f64>().map_err(|_| {
                error(
                    "invalid_double_value",
                    "double annotation value is not an f64",
                    Some(scalar.span().clone()),
                )
            })?;
            Ok(CanonicalValue::Double(contract(
                CanonicalDouble::new(value),
                scalar.span(),
            )?))
        }
        ValueTypeTag::Boolean => match spelling {
            "true" => Ok(CanonicalValue::Boolean(true)),
            "false" => Ok(CanonicalValue::Boolean(false)),
            _ => Err(error(
                "invalid_boolean_value",
                "boolean annotation value must be exactly `true` or `false`",
                Some(scalar.span().clone()),
            )),
        },
        ValueTypeTag::Date => Ok(CanonicalValue::Date(contract(
            spelling.parse::<CanonicalDate>(),
            scalar.span(),
        )?)),
        ValueTypeTag::DateTime => Ok(CanonicalValue::DateTime(contract(
            spelling.parse::<CanonicalDateTime>(),
            scalar.span(),
        )?)),
        ValueTypeTag::DateTimeTz => Ok(CanonicalValue::DateTimeTz(contract(
            parse_provider_datetime_tz(spelling),
            scalar.span(),
        )?)),
        ValueTypeTag::Decimal => Ok(CanonicalValue::Decimal(contract(
            DecimalValue::new(spelling),
            scalar.span(),
        )?)),
        ValueTypeTag::Duration => Ok(CanonicalValue::Duration(contract(
            spelling.parse::<CanonicalDuration>(),
            scalar.span(),
        )?)),
    }
}

fn parse_value_type(value: &YamlScalar) -> Result<ValueTypeTag, SchemaDiagnostics> {
    match value.value() {
        "string" => Ok(ValueTypeTag::String),
        "integer" => Ok(ValueTypeTag::Long),
        "double" => Ok(ValueTypeTag::Double),
        "boolean" => Ok(ValueTypeTag::Boolean),
        "date" => Ok(ValueTypeTag::Date),
        "datetime" => Ok(ValueTypeTag::DateTime),
        "datetime-tz" => Ok(ValueTypeTag::DateTimeTz),
        "decimal" => Ok(ValueTypeTag::Decimal),
        "duration" => Ok(ValueTypeTag::Duration),
        _ => Err(error(
            "unknown_schema_value_type",
            "value type must use a canonical TypeQL value-type token",
            Some(value.span().clone()),
        )),
    }
}

fn canonical_u64(value: &YamlScalar) -> Result<u64, SchemaDiagnostics> {
    let parsed = value.value().parse::<u64>().map_err(|_| {
        error(
            "invalid_unsigned_integer",
            "value must be an unsigned integer",
            Some(value.span().clone()),
        )
    })?;
    if parsed.to_string() != value.value() {
        return Err(error(
            "non_canonical_unsigned_integer",
            "unsigned integer is not canonically spelled",
            Some(value.span().clone()),
        ));
    }
    Ok(parsed)
}

fn strict_bool(node: &YamlNode) -> Result<bool, SchemaDiagnostics> {
    let value = scalar(node)?;
    match value.value() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(error(
            "invalid_schema_boolean",
            "schema boolean must be exactly `true` or `false`",
            Some(value.span().clone()),
        )),
    }
}

fn check_keys(mapping: &YamlMapping, allowed: &[&str]) -> Result<(), SchemaDiagnostics> {
    for entry in mapping.entries() {
        if !allowed.contains(&entry.key().value()) {
            return Err(error(
                "unknown_schema_document_key",
                format!("unknown schema key `{}`", entry.key().value()),
                Some(entry.key().span().clone()),
            ));
        }
    }
    Ok(())
}

fn entry<'a>(mapping: &'a YamlMapping, key: &str) -> Option<&'a crate::YamlMappingEntry> {
    mapping
        .entries()
        .iter()
        .find(|entry| entry.key().value() == key)
}

fn required_entry<'a>(
    mapping: &'a YamlMapping,
    key: &str,
) -> Result<&'a crate::YamlMappingEntry, SchemaDiagnostics> {
    entry(mapping, key).ok_or_else(|| {
        error(
            "missing_schema_document_key",
            format!("required schema key `{key}` is missing"),
            Some(mapping.span().clone()),
        )
    })
}

fn scalar(node: &YamlNode) -> Result<&YamlScalar, SchemaDiagnostics> {
    node.as_scalar().ok_or_else(|| {
        error(
            "schema_scalar_required",
            "schema value must be a scalar",
            Some(node.span().clone()),
        )
    })
}

fn mapping(node: &YamlNode) -> Result<&YamlMapping, SchemaDiagnostics> {
    node.as_mapping().ok_or_else(|| {
        error(
            "schema_mapping_required",
            "schema value must be a mapping",
            Some(node.span().clone()),
        )
    })
}

fn sequence(node: &YamlNode) -> Result<&crate::YamlSequence, SchemaDiagnostics> {
    node.as_sequence().ok_or_else(|| {
        error(
            "schema_sequence_required",
            "schema value must be a sequence",
            Some(node.span().clone()),
        )
    })
}

fn contract<T>(result: Result<T, Diagnostic>, span: &SourceSpan) -> Result<T, SchemaDiagnostics> {
    result.map_err(|diagnostic| {
        SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, Some(span.clone())))
    })
}

fn error(
    code: &'static str,
    message: impl Into<String>,
    primary: Option<SourceSpan>,
) -> SchemaDiagnostics {
    crate::yaml::diagnostic(DiagnosticCategory::InvalidContract, code, message, primary)
}
