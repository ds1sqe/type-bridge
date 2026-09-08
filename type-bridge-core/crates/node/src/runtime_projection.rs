//! Verified package-scoped runtime projections for generated TypeScript models.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use napi::bindgen_prelude::BigInt;
use napi::{Error, Status};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{BindingTarget, ProjectedModelForm};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{DecimalValue, ValueTypeTag};
use type_bridge_orm::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use type_bridge_orm::_dynamic::{
    DynamicAttributeMap, DynamicComparisonOp, DynamicEntityRow, DynamicExpr, DynamicRelationRow,
    DynamicRolePlayer, DynamicRolePlayerInput,
};
use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::{
    AttributeValue, Database, HydratedAttribute, InstalledRuntimeProjection, ProviderRuntimeOwner,
    ThingKind, TransactionContext, ValueType,
};

use crate::match_runtime::revalidate_diagnostic;
use crate::{
    NodeMatchSessionHandle, NodeRustDatabase, NodeRustTransactionContext, NodeValidatedThingHandle,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ModelRegistration {
    type_key: String,
    target_name: String,
    create: bool,
    reference: bool,
}

struct InstalledPackage {
    projection: InstalledRuntimeProjection,
    types_by_label: BTreeMap<String, TypeId>,
}

impl InstalledPackage {
    fn type_by_label(&self, label: &str) -> napi::Result<&TypeId> {
        self.types_by_label
            .get(label)
            .ok_or_else(|| runtime_error("provider row type is outside the installed projection"))
    }
}

/// A canonical runtime projection installed for exactly one generated package.
#[napi]
pub struct NodeRuntimeProjection {
    package: Arc<InstalledPackage>,
}

#[napi]
impl NodeRuntimeProjection {
    /// Verify projection evidence and exact generated-token coverage.
    #[napi(constructor)]
    pub fn new(
        projection_json: String,
        semantic_fingerprint_json: String,
        projection_fingerprint_json: String,
        registrations_json: String,
    ) -> napi::Result<Self> {
        let runtime = decode_runtime_projection_verified(
            projection_json.as_bytes(),
            semantic_fingerprint_json.as_bytes(),
            projection_fingerprint_json.as_bytes(),
        )
        .map_err(diagnostic_error)?;
        if runtime.target() != BindingTarget::TypeScript {
            return Err(invalid_error(
                "runtime projection does not target TypeScript",
            ));
        }
        let registrations: Vec<ModelRegistration> = serde_json::from_str(&registrations_json)
            .map_err(|error| invalid_error(format!("invalid projection registrations: {error}")))?;
        if registrations.len() != runtime.models().len() {
            return Err(invalid_error(format!(
                "projection requires exactly {} model registrations, received {}",
                runtime.models().len(),
                registrations.len()
            )));
        }
        let mut covered = BTreeSet::new();
        for registration in registrations {
            let id = type_id_from_key(&registration.type_key)?;
            if !covered.insert(id.clone()) {
                return Err(invalid_error("duplicate projected model registration"));
            }
            let model = runtime.models().get(&id).ok_or_else(|| {
                invalid_error("registration references an unknown projected model")
            })?;
            if registration.target_name != model.target_name().as_str()
                || registration.create != model.create().enabled()
                || registration.reference != model.reference_read().target_name().is_some()
            {
                return Err(invalid_error(
                    "registration does not match the projected token facets",
                ));
            }
        }
        if covered.len() != runtime.models().len() {
            return Err(invalid_error(
                "projection model registration coverage is incomplete",
            ));
        }
        let mut types_by_label = BTreeMap::new();
        for id in runtime.models().keys() {
            if types_by_label
                .insert(id.label().as_str().to_owned(), id.clone())
                .is_some()
            {
                return Err(invalid_error(
                    "projection contains duplicate provider type labels",
                ));
            }
        }
        let projection = InstalledRuntimeProjection::try_new(runtime).map_err(orm_error)?;
        Ok(Self {
            package: Arc::new(InstalledPackage {
                projection,
                types_by_label,
            }),
        })
    }

    /// Bind one exact projected model to a database-owned manager.
    #[napi(js_name = "managerForDatabase")]
    pub fn manager_for_database(
        &self,
        type_key: String,
        database: &NodeRustDatabase,
    ) -> napi::Result<NodeProjectedModelManager> {
        let type_id = manageable_type(self.package.as_ref(), &type_key)?;
        let (database, runtime) = database.handles();
        Ok(NodeProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: Some(database),
            transaction: None,
            runtime,
            filters: vec![],
        })
    }

    /// Bind one exact projected model to a borrowed transaction manager.
    #[napi(js_name = "managerForTransaction")]
    pub fn manager_for_transaction(
        &self,
        type_key: String,
        transaction: &NodeRustTransactionContext,
    ) -> napi::Result<NodeProjectedModelManager> {
        let type_id = manageable_type(self.package.as_ref(), &type_key)?;
        let (transaction, runtime) = transaction.handles();
        Ok(NodeProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: None,
            transaction: Some(transaction),
            runtime,
            filters: vec![],
        })
    }

    /// Build an opaque match session from this exact installed projection only.
    #[napi(js_name = "matchSession")]
    pub fn match_session(&self) -> napi::Result<NodeMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        Ok(NodeMatchSessionHandle::from_registry(Arc::new(registry)))
    }

    /// Resolve one exact projected entity or relation token to its provider label.
    #[napi(js_name = "matchModelType")]
    pub fn match_model_type(&self, type_key: String) -> napi::Result<String> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        Ok(id.label().as_str().to_owned())
    }

    /// Validate one generated attribute scalar through the installed Rust projection.
    #[napi(js_name = "validateAttributeValueJson")]
    pub fn validate_attribute_value_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = type_id_from_key(&type_key)?;
        if id.kind() != TypeKind::Attribute {
            return Err(invalid_error(
                "generated scalar validation requires an attribute token",
            ));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection attribute model is absent"))?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| invalid_error("projection attribute has no scalar domain"))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|error| invalid_error(format!("invalid projected scalar wire: {error}")))?;
        let value = scalar_to_attribute(&wire, projected_value_type(value_type))?;
        self.package
            .projection
            .validate_attribute_value(&id, &value)
            .map_err(|error| invalid_error(error.to_string()))
    }

    /// Validate one generated owned-field scalar through the installed Rust projection.
    #[napi(js_name = "validateFieldValueJson")]
    pub fn validate_field_value_json(
        &self,
        type_key: String,
        field_name: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection model is absent"))?;
        let field = model
            .query_tokens()
            .fields()
            .values()
            .find(|field| field.target_name().as_str() == field_name)
            .ok_or_else(|| {
                invalid_error("generated value references an unknown projected field")
            })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                .map_err(diagnostic_error)?;
        let attribute = self
            .package
            .projection
            .projection()
            .models()
            .get(&attribute_id)
            .ok_or_else(|| invalid_error("projection field attribute is absent"))?;
        let value_type = attribute
            .declaration()
            .value_type()
            .ok_or_else(|| invalid_error("projection field attribute has no scalar domain"))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|error| invalid_error(format!("invalid projected scalar wire: {error}")))?;
        let value = scalar_to_attribute(&wire, projected_value_type(value_type))?;
        self.package
            .projection
            .validate_field_value(&id, &field_name, &value)
            .map_err(|error| invalid_error(error.to_string()))
    }

    /// Revalidate a diagnostic against this exact installed projection.
    #[napi(js_name = "revalidateMatchDiagnostic")]
    pub fn revalidate_match_diagnostic(&self, diagnostic: String) -> napi::Result<String> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        revalidate_diagnostic(&registry, &diagnostic)
    }

    /// Materialize one validated match thing as the package's private wire.
    #[napi(js_name = "materializeMatchThingJson")]
    pub fn materialize_match_thing_json(
        &self,
        thing: &NodeValidatedThingHandle,
    ) -> napi::Result<String> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        let concrete = registry
            .descriptor_type_name(thing.hydrated_descriptor())
            .ok_or_else(|| runtime_error("validated result descriptor is no longer registered"))?;
        let kind = match thing.hydrated_kind() {
            ThingKind::Entity => TypeKind::Entity,
            ThingKind::Relation => TypeKind::Relation,
        };
        let id = self.package.type_by_label(&concrete)?.clone();
        if id.kind() != kind {
            return Err(runtime_error(
                "validated match thing kind disagrees with the installed projection",
            ));
        }
        let attributes = match_attributes(self.package.as_ref(), &id, thing.hydrated_attributes())?;
        let wire = match kind {
            TypeKind::Entity => hydrate_entity(
                self.package.as_ref(),
                &id,
                &DynamicEntityRow {
                    iid: Some(thing.hydrated_concept_id().to_owned()),
                    type_name: Some(concrete),
                    attributes,
                },
            )?,
            TypeKind::Relation => {
                let mut role_players = Vec::new();
                for role in thing.hydrated_roles() {
                    for player in role.players() {
                        let player_type = registry
                            .descriptor_type_name(player.concrete_descriptor())
                            .ok_or_else(|| {
                                runtime_error(
                                    "validated role-player descriptor is no longer registered",
                                )
                            })?;
                        let player_id = self.package.type_by_label(&player_type)?.clone();
                        let player_kind = match player.kind() {
                            ThingKind::Entity => TypeKind::Entity,
                            ThingKind::Relation => TypeKind::Relation,
                        };
                        if player_id.kind() != player_kind {
                            return Err(runtime_error(
                                "validated role player kind disagrees with the projection",
                            ));
                        }
                        let player_attributes = match_attributes(
                            self.package.as_ref(),
                            &player_id,
                            player.attributes(),
                        )?
                        .into_iter()
                        .map(|(name, value)| (name, attribute_json_value(&value)))
                        .collect();
                        role_players.push(DynamicRolePlayer {
                            role_name: role.role().name.clone(),
                            player_iid: Some(player.concept_id().as_str().to_owned()),
                            player_type_name: Some(player_type),
                            attributes: player_attributes,
                        });
                    }
                }
                hydrate_relation(
                    self.package.as_ref(),
                    &id,
                    &DynamicRelationRow {
                        iid: Some(thing.hydrated_concept_id().to_owned()),
                        type_name: Some(concrete),
                        attributes,
                        role_players,
                    },
                )?
            }
            TypeKind::Attribute | TypeKind::Struct => {
                unreachable!("match things are always entities or relations")
            }
        };
        serde_json::to_string(&wire).map_err(json_error)
    }
}

/// Exact CRUD manager backed only by verified projection descriptors.
#[napi]
pub struct NodeProjectedModelManager {
    package: Arc<InstalledPackage>,
    type_id: TypeId,
    database: Option<Arc<Database>>,
    transaction: Option<TransactionContext>,
    runtime: Arc<ProviderRuntimeOwner>,
    filters: Vec<DynamicExpr>,
}

#[napi]
impl NodeProjectedModelManager {
    /// Insert one exact complete value and return its hydrated private wire.
    #[napi(js_name = "insertJson")]
    pub fn insert_json(&self, instance_json: String) -> napi::Result<String> {
        let mut instance = parse_wire(&instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.insert(&attributes))
                    .map_err(orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let roles =
                    lower_roles(self.package.as_ref(), &self.type_id, &descriptor, &instance)?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.insert(&attributes, &roles))
                    .map_err(orm_error)?
            }
        };
        instance.iid = Some(iid);
        wire_json(&instance)
    }

    /// Insert exact complete values atomically and return hydrated wires in input order.
    #[napi(js_name = "insertManyJson")]
    pub fn insert_many_json(&self, batch_json: String) -> napi::Result<String> {
        self.write_many_json(&batch_json, false)
    }

    /// Insert or update one exact complete value and return its hydrated private wire.
    #[napi(js_name = "putJson")]
    pub fn put_json(&self, instance_json: String) -> napi::Result<String> {
        let mut instance = parse_wire(&instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.put_exact(&attributes))
                    .map_err(orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let roles =
                    lower_roles(self.package.as_ref(), &self.type_id, &descriptor, &instance)?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.put_exact(&attributes, &roles))
                    .map_err(orm_error)?
            }
        };
        instance.iid = Some(iid);
        wire_json(&instance)
    }

    /// Put exact complete values atomically and return hydrated wires in input order.
    #[napi(js_name = "putManyJson")]
    pub fn put_many_json(&self, batch_json: String) -> napi::Result<String> {
        self.write_many_json(&batch_json, true)
    }

    /// Replace one exact complete value already identified by its TypeDB IID.
    #[napi(js_name = "updateJson")]
    pub fn update_json(&self, iid: String, instance_json: String) -> napi::Result<String> {
        let instance = parse_wire(&instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        ensure_iid(&iid)?;
        let stored = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = self
                    .runtime
                    .block_on(manager.update_and_get_exact(&iid, &attributes))
                    .map_err(orm_error)?;
                hydrate_entity(self.package.as_ref(), &self.type_id, &row)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let roles =
                    lower_roles(self.package.as_ref(), &self.type_id, &descriptor, &instance)?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let row = self
                    .runtime
                    .block_on(manager.update_and_get_exact(&iid, &attributes, &roles))
                    .map_err(orm_error)?;
                hydrate_relation(self.package.as_ref(), &self.type_id, &row)?
            }
        };
        wire_json(&stored)
    }

    /// Delete one exact projected value by its canonical TypeDB IID.
    #[napi(js_name = "deleteByIid")]
    pub fn delete_by_iid(&self, iid: String) -> napi::Result<()> {
        ensure_iid(&iid)?;
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.delete_by_iid_exact(&iid))
                    .map_err(orm_error)?;
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.delete_by_iid_exact(&iid))
                    .map_err(orm_error)?;
            }
        }
        Ok(())
    }

    /// Return a new exact projected manager narrowed by generated attribute filters.
    #[napi(js_name = "filterJson")]
    pub fn filter_json(&self, filters_json: String) -> napi::Result<Self> {
        let filters: BTreeMap<String, Value> =
            serde_json::from_str(&filters_json).map_err(|error| {
                invalid_error(format!("invalid projected manager filters: {error}"))
            })?;
        let descriptor = self.descriptor()?;
        let attributes = match &descriptor {
            TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
            TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
        };
        let mut combined = self.filters.clone();
        combined.extend(lower_filter_values(
            self.package.as_ref(),
            attributes,
            filters,
        )?);
        Ok(Self {
            package: Arc::clone(&self.package),
            type_id: self.type_id.clone(),
            database: self.database.clone(),
            transaction: self.transaction.clone(),
            runtime: Arc::clone(&self.runtime),
            filters: combined,
        })
    }

    /// Fetch one exact projected value by IID.
    #[napi(js_name = "getByIidJson")]
    pub fn get_by_iid_json(&self, iid: String) -> napi::Result<String> {
        ensure_iid(&iid)?;
        let value = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.get_by_iid_exact(&iid))
                    .map_err(orm_error)?
                    .map(|row| hydrate_entity(self.package.as_ref(), &self.type_id, &row))
                    .transpose()?
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = self
                    .runtime
                    .block_on(manager.get_by_iid_exact(&iid))
                    .map_err(orm_error)?;
                match rows.as_slice() {
                    [] => None,
                    [row] => Some(hydrate_relation(self.package.as_ref(), &self.type_id, row)?),
                    _ => {
                        return Err(runtime_error(
                            "exact IID relation query returned multiple rows",
                        ));
                    }
                }
            }
        };
        serde_json::to_string(&value).map_err(json_error)
    }

    /// Fetch all values whose concrete type exactly matches this projection.
    #[napi(js_name = "allJson")]
    pub fn all_json(&self) -> napi::Result<String> {
        serde_json::to_string(&self.read_all_values()?).map_err(json_error)
    }

    /// Fetch the first exact filtered value, or `null` when no value matches.
    #[napi(js_name = "firstJson")]
    pub fn first_json(&self) -> napi::Result<String> {
        let value = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.first_exact_with_query(&self.filters))
                    .map_err(orm_error)?
                    .as_ref()
                    .map(|row| hydrate_entity(self.package.as_ref(), &self.type_id, row))
                    .transpose()?
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.first_exact_with_query(&self.filters))
                    .map_err(orm_error)?
                    .as_ref()
                    .map(|row| hydrate_relation(self.package.as_ref(), &self.type_id, row))
                    .transpose()?
            }
        };
        serde_json::to_string(&value).map_err(json_error)
    }

    /// Count exact filtered values.
    #[napi]
    pub fn count(&self) -> napi::Result<BigInt> {
        let count = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.count_exact_with_query(&self.filters))
                    .map_err(orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.count_exact_with_query(&self.filters))
                    .map_err(orm_error)?
            }
        };
        Ok(BigInt::from(count))
    }

    /// Return whether at least one exact filtered value exists.
    #[napi]
    pub fn exists(&self) -> napi::Result<bool> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.exists_exact_with_query(&self.filters))
                    .map_err(orm_error)
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.exists_exact_with_query(&self.filters))
                    .map_err(orm_error)
            }
        }
    }
}

impl NodeProjectedModelManager {
    fn write_many_json(&self, batch_json: &str, put: bool) -> napi::Result<String> {
        let mut instances: Vec<ProjectedWire> = serde_json::from_str(batch_json)
            .map_err(|error| invalid_error(format!("invalid projected batch wire: {error}")))?;
        for instance in &instances {
            ensure_root_wire(self.package.as_ref(), instance, &self.type_id)?;
        }
        if instances.is_empty() {
            return Ok("[]".to_owned());
        }
        let iids = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        lower_attributes(
                            self.package.as_ref(),
                            &descriptor.owned_attributes,
                            instance,
                        )
                    })
                    .collect::<napi::Result<Vec<_>>>()?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                if put {
                    self.runtime
                        .block_on(manager.put_many_exact(&items))
                        .map_err(orm_error)?
                } else {
                    self.runtime
                        .block_on(manager.insert_many(&items))
                        .map_err(orm_error)?
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            lower_attributes(
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance,
                            )?,
                            lower_roles(
                                self.package.as_ref(),
                                &self.type_id,
                                &descriptor,
                                instance,
                            )?,
                        ))
                    })
                    .collect::<napi::Result<Vec<_>>>()?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                if put {
                    self.runtime
                        .block_on(manager.put_many_exact(&items))
                        .map_err(orm_error)?
                } else {
                    self.runtime
                        .block_on(manager.insert_many(&items))
                        .map_err(orm_error)?
                }
            }
        };
        if iids.len() != instances.len() {
            return Err(runtime_error(
                "projected batch write returned an unexpected IID count",
            ));
        }
        for (instance, iid) in instances.iter_mut().zip(iids) {
            instance.iid = Some(iid);
        }
        serde_json::to_string(&instances).map_err(json_error)
    }

    fn read_all_values(&self) -> napi::Result<Vec<ProjectedWire>> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.get_exact_with_query(&self.filters, &[], None, None))
                    .map_err(orm_error)?
                    .iter()
                    .map(|row| hydrate_entity(self.package.as_ref(), &self.type_id, row))
                    .collect()
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.get_exact_with_query(&self.filters, &[], None, None))
                    .map_err(orm_error)?
                    .iter()
                    .map(|row| hydrate_relation(self.package.as_ref(), &self.type_id, row))
                    .collect()
            }
        }
    }

    fn descriptor(&self) -> napi::Result<TypeDescriptor> {
        self.package
            .projection
            .descriptor(&self.type_id)
            .cloned()
            .map_err(orm_error)
    }

    fn entity_manager(
        &self,
        descriptor: Arc<EntityDescriptor>,
    ) -> napi::Result<DynamicEntityManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicEntityManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| runtime_error("projected manager has no execution target"))?;
        Ok(DynamicEntityManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
    }

    fn relation_manager(
        &self,
        descriptor: Arc<RelationDescriptor>,
    ) -> napi::Result<DynamicRelationManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicRelationManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| runtime_error("projected manager has no execution target"))?;
        Ok(DynamicRelationManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireForm {
    Complete,
    Reference,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProjectedWire {
    type_key: String,
    form: WireForm,
    iid: Option<String>,
    value: Option<ScalarWire>,
    values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScalarWire {
    value_type: ValueTypeTag,
    value: Value,
}

fn manageable_type(package: &InstalledPackage, type_key: &str) -> napi::Result<TypeId> {
    let id = type_id_from_key(type_key)?;
    package
        .projection
        .descriptor(&id)
        .map_err(|_| invalid_error("attribute and struct tokens do not expose CRUD managers"))?;
    Ok(id)
}

fn parse_wire(value: &str) -> napi::Result<ProjectedWire> {
    serde_json::from_str(value)
        .map_err(|error| invalid_error(format!("invalid projected value wire: {error}")))
}

fn nested_wire(value: &Value) -> napi::Result<ProjectedWire> {
    serde_json::from_value(value.clone())
        .map_err(|error| invalid_error(format!("invalid nested projected value wire: {error}")))
}

fn type_id_from_key(value: &str) -> napi::Result<TypeId> {
    let id: TypeId = serde_json::from_str(value)
        .map_err(|error| invalid_error(format!("invalid canonical type key: {error}")))?;
    let canonical = to_canonical_json(&id).map_err(diagnostic_error)?;
    if canonical.as_slice() != value.as_bytes() {
        return Err(invalid_error("projected type key is not canonical JSON"));
    }
    Ok(id)
}

fn canonical_type_key(id: &TypeId) -> napi::Result<String> {
    String::from_utf8(to_canonical_json(id).map_err(diagnostic_error)?)
        .map_err(|error| runtime_error(error.to_string()))
}

fn ensure_root_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
    expected: &TypeId,
) -> napi::Result<()> {
    if wire.form != WireForm::Complete || type_id_from_key(&wire.type_key)? != *expected {
        return Err(invalid_error(
            "insert requires the manager's exact complete projected model",
        ));
    }
    if wire.value.is_some() {
        return Err(invalid_error(
            "entity and relation wires cannot carry scalar values",
        ));
    }
    ensure_wire_members(package, expected, wire)
}

fn ensure_wire_members(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> napi::Result<()> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| invalid_error("projected wire references an unknown model"))?;
    let mut allowed = BTreeSet::new();
    match wire.form {
        WireForm::Complete => {
            for field in model.create().fields() {
                let token = model
                    .query_tokens()
                    .fields()
                    .get(field.token())
                    .ok_or_else(|| runtime_error("projected create field has no query token"))?;
                allowed.insert(token.target_name().as_str());
            }
            for role in model.create().roles().values() {
                let token = model
                    .query_tokens()
                    .roles()
                    .get(role.role())
                    .ok_or_else(|| runtime_error("projected create role has no query token"))?;
                allowed.insert(token.target_name().as_str());
            }
        }
        WireForm::Reference => {
            for field in model.reference_read().key_fields() {
                let token =
                    model.query_tokens().fields().get(field).ok_or_else(|| {
                        runtime_error("projected reference key has no query token")
                    })?;
                allowed.insert(token.target_name().as_str());
            }
        }
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(invalid_error(
            "projected wire contains an unprojected member",
        ));
    }
    Ok(())
}

fn lower_attributes(
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    wire: &ProjectedWire,
) -> napi::Result<DynamicAttributeMap> {
    let mut attributes = Vec::new();
    for descriptor in descriptors {
        for value in normalized_values(
            wire.values.get(&descriptor.field_name),
            descriptor_cardinality(descriptor),
        )? {
            let attribute = nested_wire(value)?;
            let expected = package.type_by_label(&descriptor.attr_name)?;
            ensure_wire_members(package, expected, &attribute)?;
            if expected.kind() != TypeKind::Attribute
                || attribute.form != WireForm::Complete
                || type_id_from_key(&attribute.type_key)? != *expected
                || attribute.iid.is_some()
                || !attribute.values.is_empty()
            {
                return Err(invalid_error(
                    "owned field requires its exact complete attribute wrapper",
                ));
            }
            let scalar = attribute
                .value
                .as_ref()
                .ok_or_else(|| invalid_error("complete attribute wrapper has no scalar value"))?;
            attributes.push((
                descriptor.attr_name.clone(),
                scalar_to_attribute(scalar, descriptor.value_type)?,
            ));
        }
    }
    Ok(attributes)
}

fn lower_filter_values(
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    filters: BTreeMap<String, Value>,
) -> napi::Result<Vec<DynamicExpr>> {
    let mut lowered = Vec::with_capacity(filters.len());
    for (key, value) in filters {
        if matches!(key.as_str(), "iid" | "_iid" | "iid__eq" | "_iid__eq") {
            lowered.push(DynamicExpr::Iid {
                iid: projected_filter_iid(&value)?,
            });
            continue;
        }
        if matches!(key.as_str(), "iid__in" | "_iid__in") {
            let values = value.as_array().ok_or_else(|| {
                invalid_error("generated manager iid__in lookup requires an array")
            })?;
            if values.is_empty() {
                return Err(invalid_error(
                    "generated manager iid__in lookup requires at least one IID",
                ));
            }
            lowered.push(DynamicExpr::Or {
                exprs: values
                    .iter()
                    .map(|value| {
                        Ok(DynamicExpr::Iid {
                            iid: projected_filter_iid(value)?,
                        })
                    })
                    .collect::<napi::Result<Vec<_>>>()?,
            });
            continue;
        }
        // Generated target names may themselves contain a recognised lookup
        // suffix. Prefer the suffix only when its prefix is also a projected
        // field; `scoreGte__eq` selects equality on the literal `scoreGte`.
        let has_field = |name: &str| {
            descriptors
                .iter()
                .any(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
        };
        let parsed_lookup = key.rsplit_once("__");
        let (field_name, lookup) = match parsed_lookup {
            Some((field_name, lookup))
                if matches!(
                    lookup,
                    "eq" | "exact"
                        | "ne"
                        | "gt"
                        | "gte"
                        | "lt"
                        | "lte"
                        | "contains"
                        | "startswith"
                        | "endswith"
                        | "regex"
                        | "like"
                        | "in"
                        | "isnull"
                ) && has_field(field_name) =>
            {
                (field_name, lookup)
            }
            _ if has_field(&key) => (key.as_str(), "eq"),
            Some((field_name, lookup)) => (field_name, lookup),
            None => (key.as_str(), "eq"),
        };
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == field_name || descriptor.attr_name == field_name
            })
            .ok_or_else(|| {
                invalid_error(format!("unknown generated manager filter {field_name:?}"))
            })?;
        if matches!(
            lookup,
            "contains" | "startswith" | "endswith" | "regex" | "like"
        ) && descriptor.value_type != ValueType::String
        {
            return Err(invalid_error(format!(
                "unsupported generated manager lookup {lookup:?} for non-string field {field_name:?}"
            )));
        }
        if lookup == "isnull" {
            let is_null = value.as_bool().ok_or_else(|| {
                invalid_error("generated manager isnull lookup requires a boolean")
            })?;
            lowered.push(DynamicExpr::IsNull {
                attr_name: descriptor.attr_name.clone(),
                is_null,
            });
            continue;
        }
        if lookup == "in" {
            let values = value
                .as_array()
                .ok_or_else(|| invalid_error("generated manager in lookup requires an array"))?;
            if values.is_empty() {
                return Err(invalid_error(
                    "generated manager in lookup requires at least one value",
                ));
            }
            lowered.push(DynamicExpr::Or {
                exprs: values
                    .iter()
                    .map(|value| {
                        Ok(DynamicExpr::Compare {
                            attr_name: descriptor.attr_name.clone(),
                            operator: DynamicComparisonOp::Eq,
                            value: projected_filter_attribute_value(
                                package, descriptor, field_name, value,
                            )?,
                        })
                    })
                    .collect::<napi::Result<Vec<_>>>()?,
            });
            continue;
        }
        let operator = match lookup {
            "eq" | "exact" => DynamicComparisonOp::Eq,
            "ne" => DynamicComparisonOp::Neq,
            "gt" => DynamicComparisonOp::Gt,
            "gte" => DynamicComparisonOp::Gte,
            "lt" => DynamicComparisonOp::Lt,
            "lte" => DynamicComparisonOp::Lte,
            "contains" => DynamicComparisonOp::Contains,
            "startswith" => DynamicComparisonOp::StartsWith,
            "endswith" => DynamicComparisonOp::EndsWith,
            "regex" | "like" => DynamicComparisonOp::Like,
            _ => {
                return Err(invalid_error(format!(
                    "unsupported generated manager lookup {lookup:?}; expected exact, eq, ne, gt, gte, lt, lte, contains, startswith, endswith, regex, in, or isnull"
                )));
            }
        };
        lowered.push(DynamicExpr::Compare {
            attr_name: descriptor.attr_name.clone(),
            operator,
            value: projected_filter_attribute_value(package, descriptor, field_name, &value)?,
        });
    }
    Ok(lowered)
}

fn projected_filter_attribute_value(
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    field_name: &str,
    value: &Value,
) -> napi::Result<AttributeValue> {
    let wrapper = nested_wire(value)?;
    let expected = package.type_by_label(&descriptor.attr_name)?;
    ensure_wire_members(package, expected, &wrapper)?;
    if expected.kind() != TypeKind::Attribute
        || wrapper.form != WireForm::Complete
        || type_id_from_key(&wrapper.type_key)? != *expected
        || wrapper.iid.is_some()
        || !wrapper.values.is_empty()
    {
        return Err(invalid_error(format!(
            "generated manager filter {field_name:?} requires its exact attribute wrapper"
        )));
    }
    let scalar = wrapper.value.as_ref().ok_or_else(|| {
        invalid_error("complete generated manager filter wrapper has no scalar value")
    })?;
    scalar_to_attribute(scalar, descriptor.value_type)
}

fn projected_filter_iid(value: &Value) -> napi::Result<String> {
    let iid = value
        .as_str()
        .ok_or_else(|| invalid_error("generated manager IID lookup requires strings"))?;
    if !is_canonical_thing_iid(iid) {
        return Err(invalid_error(
            "generated manager IID lookup requires a canonical TypeDB thing IID",
        ));
    }
    Ok(iid.to_owned())
}

fn lower_roles(
    package: &InstalledPackage,
    relation_id: &TypeId,
    descriptor: &RelationDescriptor,
    wire: &ProjectedWire,
) -> napi::Result<Vec<DynamicRolePlayerInput>> {
    let projection = package.projection.projection();
    let model = projection
        .models()
        .get(relation_id)
        .ok_or_else(|| runtime_error("relation model is absent from its installed projection"))?;
    let mut inputs = Vec::new();
    for create in model.create().roles().values() {
        let token = model
            .query_tokens()
            .roles()
            .get(create.role())
            .ok_or_else(|| runtime_error("projected create role has no query token"))?;
        let role = descriptor
            .role(create.role().label().as_str())
            .ok_or_else(|| runtime_error("projected role has no provider descriptor"))?;
        for value in normalized_values(
            wire.values.get(token.target_name().as_str()),
            role_cardinality(role),
        )? {
            let player = nested_wire(value)?;
            let player_id = type_id_from_key(&player.type_key)?;
            ensure_wire_members(package, &player_id, &player)?;
            let allowed = create.players().iter().any(|allowed| {
                allowed.id() == &player_id && wire_form(allowed.form()) == player.form
            });
            if !allowed {
                return Err(invalid_error(
                    "role received an incompatible projected player",
                ));
            }
            let iid = player.iid.clone();
            if let Some(iid) = &iid {
                ensure_iid(iid)?;
            }
            let key = if iid.is_none() {
                projected_key(package, &player_id, &player)?
            } else {
                None
            };
            if iid.is_none() && key.is_none() {
                return Err(invalid_error(
                    "role player requires an IID or complete projected key",
                ));
            }
            inputs.push(DynamicRolePlayerInput {
                role_name: create.role().label().as_str().to_owned(),
                player_type_name: player_id.label().as_str().to_owned(),
                iid,
                key,
            });
        }
    }
    Ok(inputs)
}

fn projected_key(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> napi::Result<Option<(String, AttributeValue)>> {
    let descriptor = match package.projection.descriptor(id) {
        Ok(TypeDescriptor::Entity(descriptor)) => descriptor,
        Ok(TypeDescriptor::Relation(_)) | Err(_) => return Ok(None),
    };
    let Some(key) = descriptor.key_attribute() else {
        return Ok(None);
    };
    let Some(value) = wire
        .values
        .get(&key.field_name)
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let wrapper = nested_wire(value)?;
    let expected = package.type_by_label(&key.attr_name)?;
    ensure_wire_members(package, expected, &wrapper)?;
    if wrapper.form != WireForm::Complete || type_id_from_key(&wrapper.type_key)? != *expected {
        return Err(invalid_error(
            "projected key uses the wrong complete attribute wrapper",
        ));
    }
    let scalar = wrapper
        .value
        .as_ref()
        .ok_or_else(|| invalid_error("projected key attribute has no scalar value"))?;
    Ok(Some((
        key.attr_name.clone(),
        scalar_to_attribute(scalar, key.value_type)?,
    )))
}

fn normalized_values(
    value: Option<&Value>,
    cardinality: (u32, Option<u32>),
) -> napi::Result<Vec<&Value>> {
    let (minimum, maximum) = cardinality;
    let values = match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) if maximum != Some(1) => values.iter().collect(),
        Some(Value::Array(_)) => {
            return Err(invalid_error("scalar projected member received a sequence"));
        }
        Some(value) if maximum == Some(1) => vec![value],
        Some(_) => {
            return Err(invalid_error(
                "multi-value projected member requires a sequence",
            ));
        }
    };
    let count = u32::try_from(values.len())
        .map_err(|_| invalid_error("projected member count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(invalid_error(
            "projected member violates resolved cardinality",
        ));
    }
    Ok(values)
}

fn match_attributes(
    package: &InstalledPackage,
    id: &TypeId,
    hydrated: &[HydratedAttribute],
) -> napi::Result<DynamicAttributeMap> {
    let descriptor = package.projection.descriptor(id).map_err(orm_error)?;
    let descriptors = match &descriptor {
        TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
    };
    let mut attributes = Vec::new();
    for attribute in hydrated {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == attribute.field().name)
            .ok_or_else(|| {
                runtime_error("validated match field is outside the installed projection")
            })?;
        attributes.extend(
            attribute
                .values()
                .iter()
                .cloned()
                .map(|value| (descriptor.attr_name.clone(), value)),
        );
    }
    Ok(attributes)
}

fn attribute_json_value(value: &AttributeValue) -> Value {
    match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => Value::String(value.clone()),
        AttributeValue::Long(value) => serde_json::json!(value),
        AttributeValue::Double(value) => serde_json::json!(value),
        AttributeValue::Boolean(value) => serde_json::json!(value),
    }
}

fn hydrate_entity(
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicEntityRow,
) -> napi::Result<ProjectedWire> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .entity_descriptor(id)
        .map_err(orm_error)?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: row.iid.clone(),
        value: None,
        values: hydrate_attributes(package, &descriptor.owned_attributes, &row.attributes)?,
    })
}

fn hydrate_relation(
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicRelationRow,
) -> napi::Result<ProjectedWire> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(orm_error)?;
    let mut values = hydrate_attributes(package, &descriptor.owned_attributes, &row.attributes)?;
    let projection = package.projection.projection();
    let model = projection
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("relation model is absent from its projection"))?;
    for read in model.complete_read().roles().values() {
        let token = model
            .query_tokens()
            .roles()
            .get(read.role())
            .ok_or_else(|| runtime_error("projected read role has no query token"))?;
        let role = descriptor
            .role(read.role().label().as_str())
            .ok_or_else(|| runtime_error("projected read role has no provider descriptor"))?;
        let players = row
            .role_players
            .iter()
            .filter(|player| player.role_name == read.role().label().as_str())
            .map(|player| hydrate_player(package, read.players(), player))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_member_value(players, role_cardinality(role))?,
        );
    }
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: row.iid.clone(),
        value: None,
        values,
    })
}

fn hydrate_player(
    package: &InstalledPackage,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &DynamicRolePlayer,
) -> napi::Result<ProjectedWire> {
    let label = player
        .player_type_name
        .as_deref()
        .ok_or_else(|| runtime_error("role-player row has no concrete type label"))?;
    let id = package.type_by_label(label)?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| {
            runtime_error("role-player row type is not accepted by the projected role")
        })?;
    let attributes = package
        .projection
        .role_player_attributes(id, &player.attributes)
        .map_err(orm_error)?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            if id.kind() != TypeKind::Entity {
                return Err(runtime_error(
                    "nested complete relations are forbidden; project a reference",
                ));
            }
            hydrate_entity(
                package,
                id,
                &DynamicEntityRow {
                    iid: player.player_iid.clone(),
                    type_name: player.player_type_name.clone(),
                    attributes,
                },
            )
        }
        ProjectedModelForm::Reference => {
            let iid = player
                .player_iid
                .as_deref()
                .ok_or_else(|| runtime_error("reference role-player row has no IID"))?;
            ensure_iid(iid)?;
            let descriptors = match package.projection.descriptor(id).map_err(orm_error)? {
                TypeDescriptor::Entity(descriptor) => descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .cloned()
                    .collect::<Vec<_>>(),
                TypeDescriptor::Relation(_) => Vec::new(),
            };
            Ok(ProjectedWire {
                type_key: canonical_type_key(id)?,
                form: WireForm::Reference,
                iid: Some(iid.to_owned()),
                value: None,
                values: hydrate_attributes(package, &descriptors, &attributes)?,
            })
        }
    }
}

fn hydrate_attributes(
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &DynamicAttributeMap,
) -> napi::Result<BTreeMap<String, Value>> {
    let mut values = BTreeMap::new();
    for descriptor in descriptors {
        let wrappers = attributes
            .iter()
            .filter(|(name, _)| name == &descriptor.attr_name)
            .map(|(_, value)| attribute_wire(package, descriptor, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            descriptor.field_name.clone(),
            projected_member_value(wrappers, descriptor_cardinality(descriptor))?,
        );
    }
    Ok(values)
}

fn attribute_wire(
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    value: &AttributeValue,
) -> napi::Result<ProjectedWire> {
    let id = package.type_by_label(&descriptor.attr_name)?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: None,
        value: Some(attribute_to_scalar(value, descriptor.value_type)?),
        values: BTreeMap::new(),
    })
}

fn projected_member_value(
    values: Vec<ProjectedWire>,
    cardinality: (u32, Option<u32>),
) -> napi::Result<Value> {
    let (minimum, maximum) = cardinality;
    let count = u32::try_from(values.len())
        .map_err(|_| runtime_error("hydrated value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(runtime_error("provider row violates projected cardinality"));
    }
    if maximum == Some(1) {
        values
            .into_iter()
            .next()
            .map(serde_json::to_value)
            .transpose()
            .map_err(json_error)
            .map(|value| value.unwrap_or(Value::Null))
    } else {
        serde_json::to_value(values).map_err(json_error)
    }
}

fn scalar_to_attribute(wire: &ScalarWire, expected: ValueType) -> napi::Result<AttributeValue> {
    if wire.value_type != value_type_tag(expected) {
        return Err(invalid_error(
            "scalar envelope value_type disagrees with the projection",
        ));
    }
    let text = || {
        wire.value
            .as_str()
            .ok_or_else(|| invalid_error("scalar envelope requires a string value"))
    };
    match expected {
        ValueType::String => text().map(|value| AttributeValue::String(value.to_owned())),
        ValueType::Long => {
            let value = text()?;
            let parsed = value
                .parse::<i64>()
                .map_err(|_| invalid_error("long envelope is outside i64"))?;
            if parsed.to_string() != value {
                return Err(invalid_error("long envelope is not canonical"));
            }
            Ok(AttributeValue::Long(parsed))
        }
        ValueType::Double => wire
            .value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(AttributeValue::Double)
            .ok_or_else(|| invalid_error("double envelope requires a finite number")),
        ValueType::Boolean => wire
            .value
            .as_bool()
            .map(AttributeValue::Boolean)
            .ok_or_else(|| invalid_error("boolean envelope requires a boolean")),
        ValueType::Date => canonical_temporal::<CanonicalDate>(text()?, AttributeValue::Date),
        ValueType::DateTime => {
            canonical_temporal::<CanonicalDateTime>(text()?, AttributeValue::DateTime)
        }
        ValueType::DateTimeTz => {
            canonical_temporal::<CanonicalDateTimeTz>(text()?, AttributeValue::DateTimeTZ)
        }
        ValueType::Decimal => {
            let value = text()?;
            let canonical = DecimalValue::new(value).map_err(diagnostic_error)?;
            if canonical.as_str() != value {
                return Err(invalid_error("decimal envelope is not canonical"));
            }
            Ok(AttributeValue::Decimal(value.to_owned()))
        }
        ValueType::Duration => {
            canonical_temporal::<CanonicalDuration>(text()?, AttributeValue::Duration)
        }
    }
}

fn canonical_temporal<T>(
    value: &str,
    construct: impl FnOnce(String) -> AttributeValue,
) -> napi::Result<AttributeValue>
where
    T: FromStr<Err = type_bridge_contract::diagnostic::Diagnostic> + ToString,
{
    let canonical = value.parse::<T>().map_err(diagnostic_error)?;
    if canonical.to_string() != value {
        return Err(invalid_error("temporal envelope is not canonical"));
    }
    Ok(construct(value.to_owned()))
}

fn attribute_to_scalar(value: &AttributeValue, expected: ValueType) -> napi::Result<ScalarWire> {
    let (value_type, value) = match (value, expected) {
        (AttributeValue::String(value), ValueType::String) => {
            (ValueTypeTag::String, Value::String(value.clone()))
        }
        (AttributeValue::Long(value), ValueType::Long) => {
            (ValueTypeTag::Long, Value::String(value.to_string()))
        }
        (AttributeValue::Double(value), ValueType::Double) if value.is_finite() => (
            ValueTypeTag::Double,
            serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .ok_or_else(|| runtime_error("provider returned a non-finite double"))?,
        ),
        (AttributeValue::Boolean(value), ValueType::Boolean) => {
            (ValueTypeTag::Boolean, Value::Bool(*value))
        }
        (AttributeValue::Date(value), ValueType::Date) => {
            (ValueTypeTag::Date, Value::String(value.clone()))
        }
        (AttributeValue::DateTime(value), ValueType::DateTime) => {
            (ValueTypeTag::DateTime, Value::String(value.clone()))
        }
        (AttributeValue::DateTimeTZ(value), ValueType::DateTimeTz) => {
            (ValueTypeTag::DateTimeTz, Value::String(value.clone()))
        }
        (AttributeValue::Decimal(value), ValueType::Decimal) => {
            (ValueTypeTag::Decimal, Value::String(value.clone()))
        }
        (AttributeValue::Duration(value), ValueType::Duration) => {
            (ValueTypeTag::Duration, Value::String(value.clone()))
        }
        _ => {
            return Err(runtime_error(
                "provider attribute value type disagrees with the projection",
            ));
        }
    };
    Ok(ScalarWire { value_type, value })
}

const fn value_type_tag(value: ValueType) -> ValueTypeTag {
    match value {
        ValueType::String => ValueTypeTag::String,
        ValueType::Long => ValueTypeTag::Long,
        ValueType::Double => ValueTypeTag::Double,
        ValueType::Boolean => ValueTypeTag::Boolean,
        ValueType::Date => ValueTypeTag::Date,
        ValueType::DateTime => ValueTypeTag::DateTime,
        ValueType::DateTimeTz => ValueTypeTag::DateTimeTz,
        ValueType::Decimal => ValueTypeTag::Decimal,
        ValueType::Duration => ValueTypeTag::Duration,
    }
}

const fn projected_value_type(value: ValueTypeTag) -> ValueType {
    match value {
        ValueTypeTag::String => ValueType::String,
        ValueTypeTag::Long => ValueType::Long,
        ValueTypeTag::Double => ValueType::Double,
        ValueTypeTag::Boolean => ValueType::Boolean,
        ValueTypeTag::Date => ValueType::Date,
        ValueTypeTag::DateTime => ValueType::DateTime,
        ValueTypeTag::DateTimeTz => ValueType::DateTimeTz,
        ValueTypeTag::Decimal => ValueType::Decimal,
        ValueTypeTag::Duration => ValueType::Duration,
    }
}

const fn wire_form(value: ProjectedModelForm) -> WireForm {
    match value {
        ProjectedModelForm::Complete => WireForm::Complete,
        ProjectedModelForm::Reference => WireForm::Reference,
    }
}

fn descriptor_cardinality(descriptor: &OwnedAttributeDescriptor) -> (u32, Option<u32>) {
    descriptor
        .cardinality()
        .unwrap_or((u32::from(!descriptor.is_optional), Some(1)))
}

fn role_cardinality(descriptor: &RoleDescriptor) -> (u32, Option<u32>) {
    descriptor.cardinality.unwrap_or((0, Some(1)))
}

fn ensure_row_type(id: &TypeId, actual: Option<&str>) -> napi::Result<()> {
    if actual.is_some_and(|actual| actual != id.label().as_str()) {
        return Err(runtime_error(
            "exact provider row returned a different concrete type",
        ));
    }
    Ok(())
}

fn ensure_iid(value: &str) -> napi::Result<()> {
    if value.is_empty() {
        Err(invalid_error("IID must be a non-empty string"))
    } else {
        Ok(())
    }
}

fn wire_json(value: &ProjectedWire) -> napi::Result<String> {
    serde_json::to_string(value).map_err(json_error)
}

fn diagnostic_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    invalid_error(error.to_string())
}

fn orm_error(error: type_bridge_orm::OrmError) -> Error {
    runtime_error(error.to_string())
}

fn json_error(error: serde_json::Error) -> Error {
    runtime_error(error.to_string())
}

fn invalid_error(message: impl Into<String>) -> Error {
    Error::new(Status::InvalidArg, message.into())
}

fn runtime_error(message: impl Into<String>) -> Error {
    Error::new(Status::GenericFailure, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_envelopes_preserve_long_and_reject_noncanonical_domains() {
        let long = ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9007199254740993".into()),
        };
        assert_eq!(
            scalar_to_attribute(&long, ValueType::Long).unwrap(),
            AttributeValue::Long(9_007_199_254_740_993)
        );
        let leading_zero = ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("01".into()),
        };
        assert!(scalar_to_attribute(&leading_zero, ValueType::Long).is_err());
        let date = ScalarWire {
            value_type: ValueTypeTag::Date,
            value: Value::String("2024-02-29".into()),
        };
        assert_eq!(
            scalar_to_attribute(&date, ValueType::Date).unwrap(),
            AttributeValue::Date("2024-02-29".into())
        );
        let bad_date = ScalarWire {
            value_type: ValueTypeTag::Date,
            value: Value::String("2023-02-29".into()),
        };
        assert!(scalar_to_attribute(&bad_date, ValueType::Date).is_err());
    }
}
