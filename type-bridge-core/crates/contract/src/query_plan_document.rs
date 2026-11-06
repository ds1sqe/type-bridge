//! Schema-derived verification shared by local and staged thing fetches.
use super::*;
use crate::query_remote::RemoteFieldValue;
use crate::schema::SchemaAnnotationValue;
use crate::semantic_profile::{InterfaceKind, SemanticProfile};
use crate::value::{CanonicalValue, Cardinality};

type Fields = BTreeMap<String, RemoteFieldValue>;
type Types = BTreeMap<String, BTreeMap<String, AttributeShape>>;

/// Trusted result shape derived while validating a terminal fetch plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchDocumentShape {
    types: Types,
    roles: Vec<RoleShape>,
    scalars: BTreeMap<String, ScalarShape>,
    ordered_roles: bool,
    complete_model_roles: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScalarShape {
    domain: ValueTypeTag,
    optional: bool,
    iid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoleShape {
    required: bool,
    players: Types,
    collect: bool,
    owner: Option<std::sync::Arc<RoleOwner>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoleOwner {
    label: String,
    types: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttributeShape {
    domain: ValueTypeTag,
    cardinality: Cardinality,
    list: bool,
}

impl FetchDocumentShape {
    pub(super) fn new(
        index: &SchemaIndex<'_>,
        document: &ThingDocument,
        bindings: &BTreeMap<BindingId, BTreeSet<TypeId>>,
        scalar_types: &BTreeMap<BindingId, ValueTypeTag>,
        optional: &BTreeSet<BindingId>,
        profile: &SemanticProfile,
    ) -> Self {
        let types = index.document_types(&bindings[&document.thing], profile);
        let roles = document
            .roles
            .iter()
            .map(|listed| {
                let players = index
                    .schema
                    .facts()
                    .filter_map(|fact| {
                        let SchemaFact::Type(fact) = fact else {
                            return None;
                        };
                        let ancestors = index.ancestors(fact.id());
                        index
                            .schema
                            .facts()
                            .any(|fact| {
                                matches!(fact, SchemaFact::Plays(fact)
                    if fact.id().role() == &listed.role && ancestors.contains(fact.id().player()))
                            })
                            .then(|| fact.id().clone())
                    })
                    .collect();
                let candidates = &bindings[&document.thing];
                let owner = if candidates.len() == 1 {
                    None
                } else {
                    let owner = TypeId::new(
                        TypeKind::Relation,
                        listed.role.declaring_relation().as_str(),
                    )
                    .expect("validated role owner");
                    let belongs = |candidate: &TypeId| index.ancestors(candidate).contains(&owner);
                    if candidates.iter().all(belongs) {
                        None
                    } else {
                        Some(std::sync::Arc::new(RoleOwner {
                            label: owner.label().as_str().to_owned(),
                            types: candidates
                                .iter()
                                .filter(|candidate| belongs(candidate))
                                .map(|candidate| candidate.label().as_str().to_owned())
                                .collect(),
                        }))
                    }
                };
                RoleShape {
                    required: listed.required,
                    players: index.document_types(&players, profile),
                    collect: listed.collect,
                    owner,
                }
            })
            .collect();
        let mut ordered_roles = false;
        let mut complete_model_roles = true;
        for candidate in &bindings[&document.thing] {
            let ancestors = index.ancestors(candidate);
            for fact in index.schema.facts() {
                let SchemaFact::Relates(role) = fact else {
                    continue;
                };
                if !ancestors.contains(role.id().relation()) {
                    continue;
                }
                if !role.collection_mode().is_unordered() {
                    ordered_roles = true;
                }
                complete_model_roles &= document
                    .roles
                    .iter()
                    .any(|listed| &listed.role == role.id().role());
            }
        }
        Self {
            types,
            roles,
            ordered_roles,
            complete_model_roles,
            scalars: document
                .scalars
                .iter()
                .map(|scalar| {
                    (
                        scalar.key.as_str().to_owned(),
                        ScalarShape {
                            domain: scalar_types
                                .get(&scalar.binding)
                                .copied()
                                .unwrap_or(ValueTypeTag::String),
                            optional: optional.contains(&scalar.binding),
                            iid: bindings.get(&scalar.binding).is_some_and(|domains| {
                                !domains.is_empty()
                                    && domains.iter().all(|domain| {
                                        matches!(
                                            domain.kind(),
                                            TypeKind::Entity | TypeKind::Relation
                                        )
                                    })
                            }),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Declaring relation for an optional role that only some fetched types have.
    #[must_use]
    pub fn restricted_role_owner(&self, position: usize) -> Option<&str> {
        self.roles
            .get(position)?
            .owner
            .as_ref()
            .map(|owner| owner.label.as_str())
    }

    /// Whether any possible root type has ordered related-role semantics.
    #[must_use]
    pub const fn has_ordered_roles(&self) -> bool {
        self.ordered_roles
    }

    /// Whether one terminal fetch can represent every possible model's roles.
    #[must_use]
    pub const fn supports_write_model_projection(&self) -> bool {
        self.complete_model_roles
    }

    /// Concrete type labels admitted by this terminal fetch shape.
    pub fn type_labels(&self) -> impl Iterator<Item = &str> {
        self.types.keys().map(String::as_str)
    }

    /// Scalar domain of an owned attribute for an exact fetched type.
    /// `role` selects a listed player; `None` selects the fetched thing.
    pub fn attribute_domain(
        &self,
        role: Option<usize>,
        type_label: &str,
        attribute: &str,
    ) -> Option<ValueTypeTag> {
        let types = match role {
            Some(index) => &self.roles.get(index)?.players,
            None => &self.types,
        };
        types
            .get(type_label)?
            .get(attribute)
            .map(|shape| shape.domain)
    }

    /// Domain of one additional scalar output field.
    pub fn scalar_domain(&self, key: &str) -> Option<ValueTypeTag> {
        self.scalars.get(key).map(|scalar| scalar.domain)
    }

    /// Verify complete typed fetch evidence before materializing host objects.
    /// Optional players are either absent triples or three explicit absences.
    pub fn validate(&self, document: &Fields) -> Result<(), Diagnostic> {
        let mut allowed = BTreeSet::from([
            "_iid".to_owned(),
            "_type".to_owned(),
            "attributes".to_owned(),
        ]);
        allowed.extend(self.scalars.keys().cloned());
        for (key, scalar) in &self.scalars {
            if scalar.optional && matches!(document.get(key), None | Some(RemoteFieldValue::Absent))
            {
                continue;
            }
            let Some(RemoteFieldValue::Scalar { value }) = document.get(key) else {
                return Err(document_error("value"));
            };
            if value.value_type() != scalar.domain
                || (scalar.iid
                    && !matches!(value, CanonicalValue::String(value) if crate::id::is_canonical_thing_iid(value.as_str())))
            {
                return Err(document_error("value"));
            }
        }
        for (index, RoleShape { collect, .. }) in self.roles.iter().enumerate() {
            if *collect {
                allowed.insert(format!("_role_{index}_players"));
                continue;
            }
            for suffix in ["iid", "type", "attributes"] {
                allowed.insert(format!("_role_{index}_{suffix}"));
            }
        }
        if document.keys().any(|key| !allowed.contains(key)) {
            return Err(document_error("key"));
        }
        validate_thing(
            document.get("_iid"),
            document.get("_type"),
            document.get("attributes"),
            &self.types,
        )?;
        for (
            index,
            RoleShape {
                required,
                players: types,
                collect,
                owner,
            },
        ) in self.roles.iter().enumerate()
        {
            let applicable = owner.as_ref().is_none_or(|owner| {
                string(document.get("_type")).is_some_and(|label| owner.types.contains(label))
            });
            if *collect {
                let Some(RemoteFieldValue::Documents { documents }) =
                    document.get(&format!("_role_{index}_players"))
                else {
                    return Err(document_error("role"));
                };
                if (*required && documents.is_empty()) || (!applicable && !documents.is_empty()) {
                    return Err(document_error("role"));
                }
                let mut iids = BTreeSet::new();
                for player in documents {
                    if player.len() != 3
                        || !iids.insert(
                            string(player.get("_iid")).ok_or_else(|| document_error("role"))?,
                        )
                    {
                        return Err(document_error("role"));
                    }
                    validate_thing(
                        player.get("_iid"),
                        player.get("_type"),
                        player.get("attributes"),
                        types,
                    )
                    .map_err(|_| document_error("role"))?;
                }
                continue;
            }
            let values = ["iid", "type", "attributes"]
                .map(|suffix| document.get(&format!("_role_{index}_{suffix}")));
            if values.iter().all(Option::is_none)
                || values
                    .iter()
                    .all(|value| matches!(value, Some(RemoteFieldValue::Absent)))
            {
                if *required {
                    return Err(document_error("role"));
                }
                continue;
            }
            if values
                .iter()
                .any(|value| value.is_none() || matches!(value, Some(RemoteFieldValue::Absent)))
            {
                return Err(document_error("role"));
            }
            if !applicable {
                return Err(document_error("role"));
            }
            validate_thing(values[0], values[1], values[2], types)
                .map_err(|_| document_error("role"))?;
        }
        Ok(())
    }
}

impl SchemaIndex<'_> {
    pub(super) fn has_list_ownership(&self, type_id: &TypeId, profile: &SemanticProfile) -> bool {
        self.document_types(&BTreeSet::from([type_id.clone()]), profile)
            .values()
            .any(|fields| fields.values().any(|field| field.list))
    }

    fn document_types(&self, candidates: &BTreeSet<TypeId>, profile: &SemanticProfile) -> Types {
        candidates
            .iter()
            .filter(|type_id| {
                !self.annotation(
                    AnnotationSubjectId::Type((*type_id).clone()),
                    AnnotationKindId::Abstract,
                )
            })
            .map(|type_id| {
                let ancestors = self.ancestors(type_id);
                let attributes = ancestors
                    .iter()
                    .rev()
                    .flat_map(|owner| {
                        self.schema.facts().filter(move |fact| {
                            matches!(fact,
                        SchemaFact::Owns(owns) if owns.id().owner() == owner)
                        })
                    })
                    .filter_map(|fact| {
                        let SchemaFact::Owns(fact) = fact else {
                            return None;
                        };
                        let attribute = fact.id().attribute();
                        let domain = self.attribute_domain(attribute)?;
                        let explicit = match self.schema.fact(&SchemaFactId::Annotation(
                            AnnotationFactId::new(
                                AnnotationSubjectId::Owns(fact.id().clone()),
                                AnnotationKindId::Card,
                            ),
                        )) {
                            Some(SchemaFact::Annotation(annotation)) => match annotation.value() {
                                SchemaAnnotationValue::Cardinality(value) => Some(*value),
                                _ => unreachable!("validated cardinality annotation"),
                            },
                            _ => None,
                        };
                        let cardinality = profile.effective_cardinality(
                            InterfaceKind::Owns,
                            explicit,
                            self.annotation(
                                AnnotationSubjectId::Owns(fact.id().clone()),
                                AnnotationKindId::Key,
                            ),
                        );
                        Some((
                            attribute.label().as_str().to_owned(),
                            AttributeShape {
                                domain,
                                list: fact.collection_mode()
                                    == crate::schema::CollectionMode::OrderedList
                                    || cardinality.max().is_none_or(|max| max > 1),
                                cardinality,
                            },
                        ))
                    })
                    .collect();
                (type_id.label().as_str().to_owned(), attributes)
            })
            .collect()
    }
}

fn string(value: Option<&RemoteFieldValue>) -> Option<&str> {
    match value {
        Some(RemoteFieldValue::Scalar {
            value: CanonicalValue::String(value),
        }) => Some(value.as_str()),
        _ => None,
    }
}

fn validate_thing(
    iid: Option<&RemoteFieldValue>,
    type_name: Option<&RemoteFieldValue>,
    attributes: Option<&RemoteFieldValue>,
    types: &Types,
) -> Result<(), Diagnostic> {
    let iid = string(iid).ok_or_else(|| document_error("value"))?;
    if !crate::id::is_canonical_thing_iid(iid) {
        return Err(document_error("value"));
    }
    let attributes_by_name = string(type_name)
        .and_then(|name| types.get(name))
        .ok_or_else(|| document_error("type"))?;
    let Some(RemoteFieldValue::Document { fields }) = attributes else {
        return Err(document_error("value"));
    };
    for name in fields.keys() {
        if !attributes_by_name.contains_key(name) {
            return Err(document_error("attribute"));
        }
    }
    for (name, shape) in attributes_by_name {
        let values = match fields.get(name) {
            Some(RemoteFieldValue::List { values }) if shape.list => values.as_slice(),
            Some(RemoteFieldValue::Scalar { value }) if !shape.list => std::slice::from_ref(value),
            Some(RemoteFieldValue::Absent) if !shape.list => &[],
            None => &[],
            _ => return Err(document_error("value")),
        };
        if (values.len() as u64) < shape.cardinality.min()
            || shape
                .cardinality
                .max()
                .is_some_and(|max| values.len() as u64 > max)
            || values
                .iter()
                .any(|value| value.value_type() != shape.domain)
        {
            return Err(document_error("value"));
        }
    }
    Ok(())
}

fn document_error(kind: &str) -> Diagnostic {
    let code = match kind {
        "key" => "query_remote_v3_document_key",
        "type" => "query_remote_v3_document_type",
        "attribute" => "query_remote_v3_document_attribute",
        "role" => "query_remote_v3_document_role",
        _ => "query_remote_v3_document_value",
    };
    invalid(
        code,
        "fetch document contradicts its validated schema shape",
    )
}
