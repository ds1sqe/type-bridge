//! Explicit runtime descriptor registry.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use sha2::{Digest, Sha256};
use type_bridge_core_lib::compiler::is_valid_typeql_label;

use crate::descriptor::{EntityDescriptor, RelationDescriptor, TypeDescriptor, TypeDescriptorRef};
use crate::error::{OrmError, Result};
use crate::match_request::ids::{DescriptorId, FieldId, RoleId, SchemaFingerprint};

/// Canonical descriptor/member identities from one registry snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorIdentitySnapshot {
    /// Kind-qualified descriptor identity.
    pub descriptor_id: DescriptorId,
    /// Owner-qualified fields in canonical member-name order.
    pub fields: Vec<FieldId>,
    /// Owner-qualified roles in canonical role-name order.
    pub roles: Vec<RoleId>,
}

/// One root of a request-relevant descriptor closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorFingerprintRoot {
    /// Descriptor referenced by the request.
    pub descriptor_id: DescriptorId,
    /// Whether registered subtypes of this target affect the request.
    pub include_subtypes: bool,
}

impl DescriptorFingerprintRoot {
    /// Create one request-relevant fingerprint root.
    pub fn new(descriptor_id: DescriptorId, include_subtypes: bool) -> Self {
        Self {
            descriptor_id,
            include_subtypes,
        }
    }
}

/// Thread-safe registry for runtime entity and relation descriptors.
///
/// The registry is intentionally standalone: it has no database, transaction,
/// manager, Python, or TypeScript dependency. Bindings normalize their metadata
/// into descriptors before registration.
#[derive(Debug, Default)]
pub struct DescriptorRegistry {
    descriptors: RwLock<HashMap<String, TypeDescriptorRef>>,
}

impl DescriptorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an entity descriptor.
    ///
    /// Identical re-registration is idempotent and returns the canonical stored
    /// descriptor. Conflicting shapes or type-kind conflicts return typed ORM
    /// errors.
    pub fn register_entity(&self, descriptor: EntityDescriptor) -> Result<Arc<EntityDescriptor>> {
        validate_entity_descriptor(&descriptor)?;

        let mut descriptors = self.descriptors.write().map_err(lock_error)?;
        match descriptors.get(&descriptor.type_name) {
            Some(TypeDescriptorRef::Entity(existing)) if existing.as_ref() == &descriptor => {
                Ok(Arc::clone(existing))
            }
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "entity descriptor shape differs from registered descriptor".into(),
            }),
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "type name is already registered as a relation".into(),
            }),
            None => {
                let descriptor = Arc::new(descriptor);
                descriptors.insert(
                    descriptor.type_name.clone(),
                    TypeDescriptorRef::Entity(Arc::clone(&descriptor)),
                );
                Ok(descriptor)
            }
        }
    }

    /// Register a relation descriptor.
    ///
    /// Identical re-registration is idempotent and returns the canonical stored
    /// descriptor. Conflicting shapes or type-kind conflicts return typed ORM
    /// errors.
    pub fn register_relation(
        &self,
        descriptor: RelationDescriptor,
    ) -> Result<Arc<RelationDescriptor>> {
        validate_relation_descriptor(&descriptor)?;

        let mut descriptors = self.descriptors.write().map_err(lock_error)?;
        match descriptors.get(&descriptor.type_name) {
            Some(TypeDescriptorRef::Relation(existing)) if existing.as_ref() == &descriptor => {
                Ok(Arc::clone(existing))
            }
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "relation descriptor shape differs from registered descriptor".into(),
            }),
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "type name is already registered as an entity".into(),
            }),
            None => {
                let descriptor = Arc::new(descriptor);
                descriptors.insert(
                    descriptor.type_name.clone(),
                    TypeDescriptorRef::Relation(Arc::clone(&descriptor)),
                );
                Ok(descriptor)
            }
        }
    }

    /// Lookup an entity descriptor by TypeDB type name.
    pub fn entity(&self, type_name: &str) -> Result<Arc<EntityDescriptor>> {
        match self.get(type_name) {
            Some(TypeDescriptorRef::Entity(descriptor)) => Ok(descriptor),
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: type_name.to_string(),
                message: "requested entity but descriptor is a relation".into(),
            }),
            None => Err(OrmError::DescriptorNotFound(type_name.to_string())),
        }
    }

    /// Lookup a relation descriptor by TypeDB type name.
    pub fn relation(&self, type_name: &str) -> Result<Arc<RelationDescriptor>> {
        match self.get(type_name) {
            Some(TypeDescriptorRef::Relation(descriptor)) => Ok(descriptor),
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: type_name.to_string(),
                message: "requested relation but descriptor is an entity".into(),
            }),
            None => Err(OrmError::DescriptorNotFound(type_name.to_string())),
        }
    }

    /// Lookup any descriptor by TypeDB type name.
    pub fn get(&self, type_name: &str) -> Option<TypeDescriptorRef> {
        self.descriptors
            .read()
            .ok()
            .and_then(|descriptors| descriptors.get(type_name).cloned())
    }

    /// Return an owned snapshot of all registered descriptors.
    pub fn snapshot(&self) -> Vec<TypeDescriptor> {
        let mut descriptors: Vec<_> = self
            .descriptors
            .read()
            .map(|descriptors| {
                descriptors
                    .values()
                    .map(TypeDescriptorRef::to_owned_descriptor)
                    .collect()
            })
            .unwrap_or_default();
        descriptors.sort_by(|left, right| left.type_name().cmp(right.type_name()));
        descriptors
    }

    /// Return the deterministic kind-qualified identity for a registered type.
    pub fn descriptor_id(&self, type_name: &str) -> Option<DescriptorId> {
        self.get(type_name)
            .as_ref()
            .map(descriptor_id_for_reference)
    }

    /// Resolve one validated kind-qualified descriptor identity to its TypeDB name.
    ///
    /// Language result materializers use this after canonical result validation;
    /// they never parse the descriptor-ID spelling themselves.
    #[doc(hidden)]
    pub fn descriptor_type_name(&self, descriptor_id: &DescriptorId) -> Option<String> {
        self.descriptor_by_id(descriptor_id)
            .map(|descriptor| descriptor.type_name().to_owned())
    }

    /// Resolve the provider-facing TypeDB attribute label of a registered field.
    ///
    /// The binding-facing field name and the TypeDB attribute label may
    /// differ (renamed members); consumers emitting provider syntax must use
    /// this canonical label, never the field name.
    pub fn provider_attribute_name(&self, field: &FieldId) -> Option<String> {
        let descriptor = self.descriptor_by_id(&field.owner)?;
        let attribute = match &descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(&field.name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(&field.name),
        }?;
        Some(attribute.attr_name.clone())
    }

    /// Resolve an owner-qualified field identity by binding-facing field name
    /// or TypeDB attribute name.
    pub fn field_id(&self, owner: &DescriptorId, field_name: &str) -> Option<FieldId> {
        let descriptor = self.descriptor_by_id(owner)?;
        let attribute = match &descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(field_name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(field_name),
        }?;
        Some(FieldId::new(owner.clone(), attribute.field_name.clone()))
    }

    /// Resolve an owner-qualified role identity from a relation descriptor.
    pub fn role_id(&self, owner: &DescriptorId, role_name: &str) -> Option<RoleId> {
        let TypeDescriptorRef::Relation(descriptor) = self.descriptor_by_id(owner)? else {
            return None;
        };
        let role = descriptor.role(role_name)?;
        Some(RoleId::new(owner.clone(), role.role_name.clone()))
    }

    /// Whether a field reference owned by `reference_owner` denotes the same
    /// effective field on `binding_owner`.
    ///
    /// A parent-owned reference is valid for a registered subtype only when the
    /// subtype's flattened descriptor still contains the identical ownership.
    /// This rejects unrelated same-label fields and child shadows while keeping
    /// inherited references nominally meaningful.
    pub(crate) fn field_reference_is_compatible(
        &self,
        binding_owner: &DescriptorId,
        reference_owner: &DescriptorId,
        field_name: &str,
    ) -> bool {
        if !self.is_same_or_subtype(binding_owner, reference_owner) {
            return false;
        }
        let Some(binding_descriptor) = self.descriptor_by_id(binding_owner) else {
            return false;
        };
        let Some(reference_descriptor) = self.descriptor_by_id(reference_owner) else {
            return false;
        };
        let binding_attribute = match &binding_descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(field_name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(field_name),
        };
        let reference_attribute = match &reference_descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(field_name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(field_name),
        };
        binding_attribute.is_some() && binding_attribute == reference_attribute
    }

    /// Whether a relation-role reference owned by `reference_owner` denotes
    /// the same effective role on `binding_owner`.
    pub(crate) fn role_reference_is_compatible(
        &self,
        binding_owner: &DescriptorId,
        reference_owner: &DescriptorId,
        role_name: &str,
    ) -> bool {
        if !self.is_same_or_subtype(binding_owner, reference_owner) {
            return false;
        }
        let Some(TypeDescriptorRef::Relation(binding_descriptor)) =
            self.descriptor_by_id(binding_owner)
        else {
            return false;
        };
        let Some(TypeDescriptorRef::Relation(reference_descriptor)) =
            self.descriptor_by_id(reference_owner)
        else {
            return false;
        };
        let binding_role = binding_descriptor.role(role_name);
        let reference_role = reference_descriptor.role(role_name);
        binding_role.is_some() && binding_role == reference_role
    }

    pub(crate) fn is_same_or_subtype(
        &self,
        actual: &DescriptorId,
        expected: &DescriptorId,
    ) -> bool {
        let Some(mut current) = self.descriptor_by_id(actual) else {
            return false;
        };
        let Some(expected_descriptor) = self.descriptor_by_id(expected) else {
            return false;
        };
        if !same_descriptor_kind(&current, &expected_descriptor) {
            return false;
        }

        let expected_name = expected_descriptor.type_name();
        let mut visited = BTreeSet::new();
        loop {
            if current.type_name() == expected_name {
                return true;
            }
            if !visited.insert(current.type_name().to_owned()) {
                return false;
            }
            let Some(parent) = descriptor_parent_ref(&current) else {
                return false;
            };
            let Some(parent_descriptor) = self.get(parent) else {
                return false;
            };
            if !same_descriptor_kind(&current, &parent_descriptor) {
                return false;
            }
            current = parent_descriptor;
        }
    }

    /// Return a deterministic descriptor/member identity snapshot.
    ///
    /// Registration order, hash-map iteration order, and allocation addresses
    /// cannot affect this result.
    pub fn identity_snapshot(&self) -> Result<Vec<DescriptorIdentitySnapshot>> {
        let descriptors = self.descriptors.read().map_err(lock_error)?;
        let mut snapshot: Vec<_> = descriptors
            .values()
            .map(|descriptor| {
                let descriptor_id = descriptor_id_for_reference(descriptor);
                let mut fields = descriptor_attributes(descriptor)
                    .iter()
                    .map(|attribute| {
                        FieldId::new(descriptor_id.clone(), attribute.field_name.clone())
                    })
                    .collect::<Vec<_>>();
                fields.sort();

                let mut roles = match descriptor {
                    TypeDescriptorRef::Entity(_) => Vec::new(),
                    TypeDescriptorRef::Relation(relation) => relation
                        .roles
                        .iter()
                        .map(|role| RoleId::new(descriptor_id.clone(), role.role_name.clone()))
                        .collect(),
                };
                roles.sort();

                DescriptorIdentitySnapshot {
                    descriptor_id,
                    fields,
                    roles,
                }
            })
            .collect();
        snapshot.sort_by(|left, right| left.descriptor_id.cmp(&right.descriptor_id));
        Ok(snapshot)
    }

    /// Fingerprint the complete registered descriptor snapshot.
    pub fn schema_fingerprint(&self) -> Result<SchemaFingerprint> {
        let descriptors = self.owned_snapshot()?;
        Ok(fingerprint_descriptors(descriptors.values()))
    }

    /// Fingerprint only the descriptor facts relevant to the supplied roots.
    ///
    /// Every root includes its registered ancestors. Roots marked
    /// `include_subtypes` also include their registered subtype closure. Role
    /// player types, their ancestors, and their registered subtypes are always
    /// included because those facts determine role-player compatibility.
    /// Fields and effective relation roles are encoded in each included
    /// descriptor. An unrelated registration therefore cannot stale this
    /// fingerprint.
    pub fn request_relevant_fingerprint(
        &self,
        roots: &[DescriptorFingerprintRoot],
    ) -> Result<SchemaFingerprint> {
        let descriptors = self.owned_snapshot()?;
        let mut included = BTreeSet::new();
        let mut subtype_closure = BTreeSet::new();

        for root in roots {
            let type_name = resolve_descriptor_id(&descriptors, &root.descriptor_id)?;
            included.insert(type_name.clone());
            if root.include_subtypes {
                subtype_closure.insert(type_name);
            }
        }

        loop {
            let mut changed = false;
            let current: Vec<_> = included.iter().cloned().collect();

            for type_name in current {
                let descriptor = descriptors
                    .get(&type_name)
                    .ok_or_else(|| OrmError::DescriptorNotFound(type_name.clone()))?;

                if let Some(parent_type) = descriptor_parent(descriptor) {
                    require_registered(&descriptors, parent_type)?;
                    changed |= included.insert(parent_type.to_string());
                }

                if let TypeDescriptor::Relation(relation) = descriptor {
                    for role in &relation.roles {
                        for player_type in &role.player_type_names {
                            require_registered(&descriptors, player_type)?;
                            changed |= included.insert(player_type.clone());
                            changed |= subtype_closure.insert(player_type.clone());
                        }
                    }
                }
            }

            let subtype_parents = subtype_closure.clone();
            for (type_name, descriptor) in &descriptors {
                if descriptor_parent(descriptor)
                    .is_some_and(|parent| subtype_parents.contains(parent))
                {
                    changed |= included.insert(type_name.clone());
                    changed |= subtype_closure.insert(type_name.clone());
                }
            }

            if !changed {
                break;
            }
        }

        Ok(fingerprint_descriptors(
            included
                .iter()
                .filter_map(|type_name| descriptors.get(type_name)),
        ))
    }

    fn descriptor_by_id(&self, descriptor_id: &DescriptorId) -> Option<TypeDescriptorRef> {
        self.descriptors.read().ok().and_then(|descriptors| {
            descriptors
                .values()
                .find(|descriptor| descriptor_id_for_reference(descriptor) == *descriptor_id)
                .cloned()
        })
    }

    fn owned_snapshot(&self) -> Result<BTreeMap<String, TypeDescriptor>> {
        let descriptors = self.descriptors.read().map_err(lock_error)?;
        Ok(descriptors
            .iter()
            .map(|(type_name, descriptor)| (type_name.clone(), descriptor.to_owned_descriptor()))
            .collect())
    }
}

fn descriptor_id_for_reference(descriptor: &TypeDescriptorRef) -> DescriptorId {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => {
            DescriptorId::new(format!("entity:{}", descriptor.type_name))
        }
        TypeDescriptorRef::Relation(descriptor) => {
            DescriptorId::new(format!("relation:{}", descriptor.type_name))
        }
    }
}

fn descriptor_id_for_owned(descriptor: &TypeDescriptor) -> DescriptorId {
    match descriptor {
        TypeDescriptor::Entity(descriptor) => {
            DescriptorId::new(format!("entity:{}", descriptor.type_name))
        }
        TypeDescriptor::Relation(descriptor) => {
            DescriptorId::new(format!("relation:{}", descriptor.type_name))
        }
    }
}

fn descriptor_attributes(
    descriptor: &TypeDescriptorRef,
) -> &[crate::descriptor::OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

fn descriptor_parent_ref(descriptor: &TypeDescriptorRef) -> Option<&str> {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => descriptor.parent_type.as_deref(),
        TypeDescriptorRef::Relation(descriptor) => descriptor.parent_type.as_deref(),
    }
}

fn same_descriptor_kind(left: &TypeDescriptorRef, right: &TypeDescriptorRef) -> bool {
    matches!(
        (left, right),
        (TypeDescriptorRef::Entity(_), TypeDescriptorRef::Entity(_))
            | (
                TypeDescriptorRef::Relation(_),
                TypeDescriptorRef::Relation(_)
            )
    )
}

fn descriptor_parent(descriptor: &TypeDescriptor) -> Option<&str> {
    match descriptor {
        TypeDescriptor::Entity(descriptor) => descriptor.parent_type.as_deref(),
        TypeDescriptor::Relation(descriptor) => descriptor.parent_type.as_deref(),
    }
}

fn resolve_descriptor_id(
    descriptors: &BTreeMap<String, TypeDescriptor>,
    descriptor_id: &DescriptorId,
) -> Result<String> {
    descriptors
        .iter()
        .find_map(|(type_name, descriptor)| {
            (descriptor_id_for_owned(descriptor) == *descriptor_id).then(|| type_name.clone())
        })
        .ok_or_else(|| OrmError::DescriptorNotFound(descriptor_id.as_str().to_string()))
}

fn require_registered(
    descriptors: &BTreeMap<String, TypeDescriptor>,
    type_name: &str,
) -> Result<()> {
    if descriptors.contains_key(type_name) {
        Ok(())
    } else {
        Err(OrmError::DescriptorNotFound(type_name.to_string()))
    }
}

fn fingerprint_descriptors<'a>(
    descriptors: impl IntoIterator<Item = &'a TypeDescriptor>,
) -> SchemaFingerprint {
    let mut descriptors: Vec<_> = descriptors.into_iter().collect();
    descriptors.sort_by_key(|descriptor| descriptor_id_for_owned(descriptor));

    let mut records = Vec::new();
    for descriptor in descriptors {
        append_descriptor_records(descriptor, &mut records);
    }
    records.sort();

    let payload = records.join("\n");
    let digest = Sha256::digest(payload.as_bytes());
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    SchemaFingerprint::new(format!("schema-sha256-v1:{digest}"))
}

fn append_descriptor_records(descriptor: &TypeDescriptor, records: &mut Vec<String>) {
    let descriptor_id = descriptor_id_for_owned(descriptor);
    let (kind, is_abstract, parent_type, attributes) = match descriptor {
        TypeDescriptor::Entity(descriptor) => (
            "entity",
            descriptor.is_abstract,
            descriptor.parent_type.as_deref(),
            descriptor.owned_attributes.as_slice(),
        ),
        TypeDescriptor::Relation(descriptor) => (
            "relation",
            descriptor.is_abstract,
            descriptor.parent_type.as_deref(),
            descriptor.owned_attributes.as_slice(),
        ),
    };
    records.push(canonical_record(&[
        "descriptor",
        descriptor_id.as_str(),
        kind,
        bool_text(is_abstract),
        parent_type.unwrap_or(""),
    ]));

    let mut attributes: Vec<_> = attributes.iter().collect();
    attributes.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    for attribute in attributes {
        let mut annotations: Vec<_> = attribute
            .annotations
            .iter()
            .map(|annotation| {
                serde_json::to_string(annotation).expect("annotation serialization cannot fail")
            })
            .collect();
        annotations.sort();
        records.push(canonical_record(&[
            "field",
            descriptor_id.as_str(),
            &attribute.field_name,
            &attribute.attr_name,
            &serde_json::to_string(&attribute.value_type)
                .expect("value-type serialization cannot fail"),
            bool_text(attribute.is_optional),
            bool_text(attribute.is_ordered),
            &canonical_list(&annotations),
        ]));
    }

    if let TypeDescriptor::Relation(relation) = descriptor {
        let mut roles: Vec<_> = relation.roles.iter().collect();
        roles.sort_by(|left, right| left.role_name.cmp(&right.role_name));
        for role in roles {
            let mut player_types = role.player_type_names.clone();
            player_types.sort();
            records.push(canonical_record(&[
                "role",
                descriptor_id.as_str(),
                &role.role_name,
                &canonical_list(&player_types),
                &cardinality_text(role.cardinality),
                role.overrides.as_deref().unwrap_or(""),
                bool_text(role.is_abstract),
                bool_text(role.ordered),
                bool_text(role.distinct),
                &cardinality_text(role.plays_cardinality),
            ]));
        }
    }
}

fn canonical_record(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("|")
}

fn canonical_list(parts: &[String]) -> String {
    canonical_record(&parts.iter().map(String::as_str).collect::<Vec<_>>())
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn cardinality_text(cardinality: Option<(u32, Option<u32>)>) -> String {
    match cardinality {
        None => "none".to_string(),
        Some((minimum, Some(maximum))) => format!("{minimum}..{maximum}"),
        Some((minimum, None)) => format!("{minimum}.."),
    }
}

fn validate_entity_descriptor(descriptor: &EntityDescriptor) -> Result<()> {
    validate_type_name(&descriptor.type_name)?;
    if let Some(parent_type) = &descriptor.parent_type {
        validate_type_name(parent_type)?;
    }
    validate_attributes(&descriptor.type_name, &descriptor.owned_attributes)
}

fn validate_relation_descriptor(descriptor: &RelationDescriptor) -> Result<()> {
    validate_type_name(&descriptor.type_name)?;
    if let Some(parent_type) = &descriptor.parent_type {
        validate_type_name(parent_type)?;
    }
    validate_attributes(&descriptor.type_name, &descriptor.owned_attributes)?;

    let mut role_names = HashSet::new();
    for role in &descriptor.roles {
        validate_typeql_label(&descriptor.type_name, "role name", &role.role_name)?;
        if !role_names.insert(role.role_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: descriptor.type_name.clone(),
                message: format!("duplicate role name '{}'", role.role_name),
            });
        }
        for player_type_name in &role.player_type_names {
            validate_type_name(player_type_name)?;
        }
        if let Some(overrides) = &role.overrides {
            validate_typeql_label(&descriptor.type_name, "overridden role name", overrides)?;
        }
    }

    Ok(())
}

fn validate_attributes(
    type_name: &str,
    attributes: &[crate::descriptor::OwnedAttributeDescriptor],
) -> Result<()> {
    let mut field_names = HashSet::new();
    let mut attr_names = HashSet::new();
    for attr in attributes {
        validate_non_empty(type_name, "field name", &attr.field_name)?;
        validate_typeql_label(type_name, "attribute name", &attr.attr_name)?;
        if !field_names.insert(attr.field_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: type_name.to_string(),
                message: format!("duplicate field name '{}'", attr.field_name),
            });
        }
        if !attr_names.insert(attr.attr_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: type_name.to_string(),
                message: format!("duplicate attribute name '{}'", attr.attr_name),
            });
        }
    }
    let collision = attributes
        .iter()
        .enumerate()
        .flat_map(|(field_index, field)| {
            attributes
                .iter()
                .enumerate()
                .filter(move |(attribute_index, attribute)| {
                    field_index != *attribute_index && field.field_name == attribute.attr_name
                })
                .map(move |(_, attribute)| {
                    (
                        field.field_name.as_str(),
                        field.attr_name.as_str(),
                        attribute.field_name.as_str(),
                    )
                })
        })
        .min();
    if let Some((name, field_attribute, conflicting_field)) = collision {
        return Err(OrmError::DescriptorValidation {
            type_name: type_name.to_string(),
            message: format!(
                "field name '{name}' (attribute '{field_attribute}') conflicts with attribute name '{name}' declared by field '{conflicting_field}'"
            ),
        });
    }
    Ok(())
}

fn validate_type_name(type_name: &str) -> Result<()> {
    validate_typeql_label(type_name, "type name", type_name)
}

fn validate_typeql_label(type_name: &str, label: &str, value: &str) -> Result<()> {
    if !is_valid_typeql_label(value) {
        return Err(OrmError::DescriptorValidation {
            type_name: type_name.to_string(),
            message: format!("{label} {value:?} is not a canonical TypeQL label"),
        });
    }
    Ok(())
}

fn validate_non_empty(type_name: &str, label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(OrmError::DescriptorValidation {
            type_name: type_name.to_string(),
            message: format!("{label} cannot be empty"),
        });
    }
    Ok(())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: "<registry>".into(),
        message: "descriptor registry lock is poisoned".into(),
    }
}
