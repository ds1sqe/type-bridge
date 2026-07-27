//! Verified package-scoped runtime projections for generated TypeScript models.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use napi::{Error, Status};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectedModelForm};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{DecimalValue, ValueTypeTag};
use type_bridge_orm::descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use type_bridge_orm::dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput,
};
use type_bridge_orm::manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::{
    AttributeValue, Database, InstalledRuntimeProjection, ProviderRuntimeOwner, TransactionContext,
    ValueType,
};

use crate::{NodeRustDatabase, NodeRustTransactionContext};

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
        })
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
        let values = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.all_exact())
                    .map_err(orm_error)?
                    .iter()
                    .map(|row| hydrate_entity(self.package.as_ref(), &self.type_id, row))
                    .collect::<napi::Result<Vec<_>>>()?
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime
                    .block_on(manager.all_exact())
                    .map_err(orm_error)?
                    .iter()
                    .map(|row| hydrate_relation(self.package.as_ref(), &self.type_id, row))
                    .collect::<napi::Result<Vec<_>>>()?
            }
        };
        serde_json::to_string(&values).map_err(json_error)
    }
}

impl NodeProjectedModelManager {
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
            return Ok(DynamicEntityManager::with_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| runtime_error("projected manager has no execution target"))?;
        Ok(DynamicEntityManager::new(database.as_ref(), descriptor))
    }

    fn relation_manager(
        &self,
        descriptor: Arc<RelationDescriptor>,
    ) -> napi::Result<DynamicRelationManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicRelationManager::with_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| runtime_error("projected manager has no execution target"))?;
        Ok(DynamicRelationManager::new(database.as_ref(), descriptor))
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
