//! Verified package-scoped runtime projections for generated Python models.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType};
use pythonize::pythonize;
use tokio::runtime::Runtime;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::ProjectedModelForm;
#[cfg(test)]
use type_bridge_contract::projection::RuntimeProjection;
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_orm::attribute::ValueType;
use type_bridge_orm::descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use type_bridge_orm::dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput,
};
use type_bridge_orm::manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::session::{Database, TransactionContext};
use type_bridge_orm::value::AttributeValue;
use type_bridge_orm::InstalledRuntimeProjection;

use crate::orm_runtime::{PyRustDatabase, PyRustTransactionContext};

struct RegisteredModel {
    complete: Py<PyType>,
    reference: Option<Py<PyType>>,
}

struct InstalledPackage {
    projection: InstalledRuntimeProjection,
    models: BTreeMap<TypeId, RegisteredModel>,
    types_by_label: BTreeMap<String, TypeId>,
}

impl InstalledPackage {
    fn model_id_for_class(
        &self,
        py: Python<'_>,
        class: &Py<PyType>,
    ) -> PyResult<TypeId> {
        let pointer = class.bind(py).as_ptr();
        self.models
            .iter()
            .find_map(|(id, registered)| {
                (registered.complete.bind(py).as_ptr() == pointer).then(|| id.clone())
            })
            .ok_or_else(|| py_type_error("model class is not registered in this runtime projection"))
    }

    fn identify_value(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<(TypeId, ProjectedModelForm)> {
        let pointer = value.get_type().as_ptr();
        for (id, registered) in &self.models {
            if registered.complete.bind(py).as_ptr() == pointer {
                return Ok((id.clone(), ProjectedModelForm::Complete));
            }
            if registered
                .reference
                .as_ref()
                .is_some_and(|reference| reference.bind(py).as_ptr() == pointer)
            {
                return Ok((id.clone(), ProjectedModelForm::Reference));
            }
        }
        Err(py_type_error(
            "projected value is not an exact registered complete or reference class",
        ))
    }

    fn class(
        &self,
        id: &TypeId,
        form: ProjectedModelForm,
    ) -> PyResult<&Py<PyType>> {
        let registered = self
            .models
            .get(id)
            .ok_or_else(|| py_runtime_error("projection model class is not installed"))?;
        match form {
            ProjectedModelForm::Complete => Ok(&registered.complete),
            ProjectedModelForm::Reference => registered.reference.as_ref().ok_or_else(|| {
                py_runtime_error("projection requested an unregistered reference class")
            }),
        }
    }

    fn type_by_label(&self, label: &str, kind: TypeKind) -> PyResult<&TypeId> {
        self.types_by_label
            .get(label)
            .filter(|id| id.kind() == kind)
            .ok_or_else(|| py_runtime_error(format!("projection has no {kind:?} type {label:?}")))
    }
}

/// A verified runtime projection installed for exactly one generated package.
#[pyclass]
pub struct PyRuntimeProjection {
    package: Arc<InstalledPackage>,
}

#[pymethods]
impl PyRuntimeProjection {
    /// Verify canonical projection bytes and install their exact generated classes.
    #[new]
    fn new(
        py: Python<'_>,
        projection_json: &str,
        semantic_fingerprint_json: &str,
        projection_fingerprint_json: &str,
        models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
    ) -> PyResult<Self> {
        install_projection(
            py,
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            models,
        )
        .map(|package| Self { package })
    }

    /// Bind an exact generated model class to an existing Rust database handle.
    fn manager_for_database(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        database: &PyRustDatabase,
    ) -> PyResult<PyProjectedModelManager> {
        let type_id = self.package.model_id_for_class(py, &model)?;
        ensure_manageable(self.package.as_ref(), &type_id)?;
        let (database, runtime) = database.handles();
        Ok(PyProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: Some(database),
            transaction: None,
            runtime,
        })
    }

    /// Bind an exact generated model class to an existing Rust transaction handle.
    fn manager_for_transaction(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        transaction: &PyRustTransactionContext,
    ) -> PyResult<PyProjectedModelManager> {
        let type_id = self.package.model_id_for_class(py, &model)?;
        ensure_manageable(self.package.as_ref(), &type_id)?;
        let (transaction, runtime) = transaction.handles();
        Ok(PyProjectedModelManager {
            package: Arc::clone(&self.package),
            type_id,
            database: None,
            transaction: Some(transaction),
            runtime,
        })
    }
}

/// Exact CRUD manager for one generated projected entity or relation class.
#[pyclass]
pub struct PyProjectedModelManager {
    package: Arc<InstalledPackage>,
    type_id: TypeId,
    database: Option<Arc<Database>>,
    transaction: Option<TransactionContext>,
    runtime: Arc<Runtime>,
}

#[pymethods]
impl PyProjectedModelManager {
    /// Insert one exact generated model and attach the returned TypeDB IID.
    fn insert(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<PyObject> {
        self.ensure_instance(py, &instance)?;
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(py, self.package.as_ref(), &descriptor.owned_attributes, &instance)?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                self.runtime.block_on(manager.insert(&attributes)).map_err(py_orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(py, self.package.as_ref(), &descriptor.owned_attributes, &instance)?;
                let players = lower_roles(py, self.package.as_ref(), &self.type_id, &descriptor, &instance)?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                self.runtime.block_on(manager.insert(&attributes, &players)).map_err(py_orm_error)?
            }
        };
        instance.call_method1("attach_runtime_iid", (iid,))?;
        Ok(instance.unbind())
    }

    /// Fetch all exact instances of this projected type using `isa!`.
    fn all(&self, py: Python<'_>) -> PyResult<PyObject> {
        let values = PyList::empty(py);
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let rows = self.runtime.block_on(manager.all_exact()).map_err(py_orm_error)?;
                for row in rows {
                    values.append(hydrate_entity(py, self.package.as_ref(), &self.type_id, &row)?)?;
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = self.runtime.block_on(manager.all_exact()).map_err(py_orm_error)?;
                for row in rows {
                    values.append(hydrate_relation(py, self.package.as_ref(), &self.type_id, &row)?)?;
                }
            }
        }
        Ok(values.into_any().unbind())
    }

    /// Fetch one exact instance by TypeDB IID using `isa!`.
    fn get_by_iid(&self, py: Python<'_>, iid: &str) -> PyResult<PyObject> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = self
                    .runtime
                    .block_on(manager.get_by_iid_exact(iid))
                    .map_err(py_orm_error)?;
                match row {
                    Some(row) => hydrate_entity(py, self.package.as_ref(), &self.type_id, &row),
                    None => Ok(py.None()),
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = self
                    .runtime
                    .block_on(manager.get_by_iid_exact(iid))
                    .map_err(py_orm_error)?;
                match rows.as_slice() {
                    [] => Ok(py.None()),
                    [row] => hydrate_relation(py, self.package.as_ref(), &self.type_id, row),
                    _ => Err(py_runtime_error("exact IID relation query returned multiple rows")),
                }
            }
        }
    }
}

impl PyProjectedModelManager {
    fn descriptor(&self) -> PyResult<TypeDescriptor> {
        self.package
            .projection
            .descriptor(&self.type_id)
            .cloned()
            .map_err(py_orm_error)
    }

    fn ensure_instance(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let expected = self.package.class(&self.type_id, ProjectedModelForm::Complete)?;
        if value.get_type().as_ptr() != expected.bind(py).as_ptr() {
            return Err(py_type_error(
                "insert requires an instance of the manager's exact registered class",
            ));
        }
        Ok(())
    }

    fn entity_manager(&self, descriptor: Arc<EntityDescriptor>) -> PyResult<DynamicEntityManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicEntityManager::with_transaction(transaction.clone(), descriptor));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicEntityManager::new(database.as_ref(), descriptor))
    }

    fn relation_manager(&self, descriptor: Arc<RelationDescriptor>) -> PyResult<DynamicRelationManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicRelationManager::with_transaction(transaction.clone(), descriptor));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicRelationManager::new(database.as_ref(), descriptor))
    }
}

fn install_projection(
    py: Python<'_>,
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
) -> PyResult<Arc<InstalledPackage>> {
    let runtime = decode_runtime_projection_verified(
        projection_json.as_bytes(),
        semantic_fingerprint_json.as_bytes(),
        projection_fingerprint_json.as_bytes(),
    )
    .map_err(py_diagnostic)?;
    let mut expected = BTreeMap::new();
    let mut types_by_label = BTreeMap::new();
    for (id, model) in runtime.models() {
        expected.insert(canonical_id(id)?, (id.clone(), model.target_name().as_str().to_owned(), model.reference_read().target_name().map(|name| name.as_str().to_owned())));
        if types_by_label.insert(id.label().as_str().to_owned(), id.clone()).is_some() {
            return Err(py_runtime_error("projection contains duplicate type labels"));
        }
    }
    if models.len() != expected.len() {
        return Err(py_value_error(format!(
            "projection requires exactly {} model registrations, received {}",
            expected.len(), models.len()
        )));
    }
    let mut registered = BTreeMap::new();
    let mut pointers = BTreeSet::new();
    for (complete, reference) in models {
        let id_text: String = complete.bind(py).getattr("__type_id__")?.extract()?;
        let (id, complete_name, reference_name) = expected
            .remove(&id_text)
            .ok_or_else(|| py_value_error("registered model has an unknown or duplicate __type_id__"))?;
        verify_class(py, &complete, &complete_name, "complete", &mut pointers)?;
        match (&reference_name, &reference) {
            (Some(expected_name), Some(class)) => {
                verify_class(py, class, expected_name, "reference", &mut pointers)?;
            }
            (None, None) => {}
            (Some(_), None) => return Err(py_value_error("projection reference class is missing")),
            (None, Some(_)) => return Err(py_value_error("unexpected projection reference class")),
        }
        registered.insert(id, RegisteredModel { complete, reference });
    }
    if !expected.is_empty() {
        return Err(py_value_error("projection model registration coverage is incomplete"));
    }
    let installed = InstalledRuntimeProjection::try_new(runtime).map_err(py_orm_error)?;
    Ok(Arc::new(InstalledPackage {
        projection: installed,
        models: registered,
        types_by_label,
    }))
}

fn verify_class(
    py: Python<'_>,
    class: &Py<PyType>,
    expected_name: &str,
    expected_form: &str,
    pointers: &mut BTreeSet<usize>,
) -> PyResult<()> {
    let class = class.bind(py);
    let name: String = class.getattr("__name__")?.extract()?;
    let form: String = class.getattr("__model_form__")?.extract()?;
    if name != expected_name || form != expected_form {
        return Err(py_value_error(format!(
            "registered class {name:?} does not match projected {expected_form} class {expected_name:?}"
        )));
    }
    if !pointers.insert(class.as_ptr() as usize) {
        return Err(py_value_error("one Python class was registered for multiple projected forms"));
    }
    Ok(())
}

fn canonical_id(id: &TypeId) -> PyResult<String> {
    String::from_utf8(to_canonical_json(id).map_err(py_diagnostic)?)
        .map_err(|error| py_runtime_error(error.to_string()))
}

fn ensure_manageable(package: &InstalledPackage, id: &TypeId) -> PyResult<()> {
    if package.projection.descriptor(id).is_err() {
        return Err(py_type_error("attribute projections do not expose CRUD managers"));
    }
    Ok(())
}

fn lower_attributes(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    instance: &Bound<'_, PyAny>,
) -> PyResult<DynamicAttributeMap> {
    let values = instance.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let mut attributes = Vec::new();
    for descriptor in descriptors {
        let value = values.get_item(&descriptor.field_name)?;
        let items = normalized_items(value.as_ref(), descriptor_cardinality(descriptor))?;
        for item in items {
            let (id, form) = package.identify_value(py, &item)?;
            let expected = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
            if &id != expected || form != ProjectedModelForm::Complete {
                return Err(py_type_error(format!(
                    "field {:?} requires its exact complete attribute wrapper",
                    descriptor.field_name
                )));
            }
            let scalar = item.call_method0("runtime_attribute_value")?;
            attributes.push((
                descriptor.attr_name.clone(),
                attribute_value_from_py(py, &scalar, descriptor.value_type)?,
            ));
        }
    }
    Ok(attributes)
}

fn lower_roles(
    py: Python<'_>,
    package: &InstalledPackage,
    relation_id: &TypeId,
    descriptor: &RelationDescriptor,
    instance: &Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicRolePlayerInput>> {
    let projection = package.projection.projection();
    let model = &projection.models()[relation_id];
    let values = instance.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let mut inputs = Vec::new();
    for create in model.create().roles().values() {
        let token = &model.query_tokens().roles()[create.role()];
        let role_name = create.role().label().as_str();
        let role = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("projected role has no provider descriptor"))?;
        let value = values.get_item(token.target_name().as_str())?;
        for item in normalized_items(value.as_ref(), role_cardinality(role))? {
            let (player_id, form) = package.identify_value(py, &item)?;
            if !create
                .players()
                .iter()
                .any(|allowed| allowed.id() == &player_id && allowed.form() == form)
            {
                return Err(py_type_error(format!(
                    "role {:?} received an incompatible projected player",
                    token.target_name().as_str()
                )));
            }
            let iid = projected_iid(&item)?;
            let key = if iid.is_none() {
                projected_key(py, package, &player_id, &item)?
            } else {
                None
            };
            if iid.is_none() && key.is_none() {
                return Err(py_value_error(format!(
                    "role player {:?} requires an attached IID or projected key",
                    player_id.label().as_str()
                )));
            }
            inputs.push(DynamicRolePlayerInput {
                role_name: role_name.to_owned(),
                player_type_name: player_id.label().as_str().to_owned(),
                iid,
                key,
            });
        }
    }
    Ok(inputs)
}

fn projected_key(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    value: &Bound<'_, PyAny>,
) -> PyResult<Option<(String, AttributeValue)>> {
    let descriptor = match package.projection.descriptor(id) {
        Ok(TypeDescriptor::Entity(descriptor)) => descriptor,
        Ok(TypeDescriptor::Relation(_)) | Err(_) => return Ok(None),
    };
    let Some(key) = descriptor.key_attribute() else { return Ok(None) };
    let values = value.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let Some(wrapper) = values.get_item(&key.field_name)? else { return Ok(None) };
    if wrapper.is_none() {
        return Ok(None);
    }
    let (wrapper_id, form) = package.identify_value(py, &wrapper)?;
    let expected = package.type_by_label(&key.attr_name, TypeKind::Attribute)?;
    if &wrapper_id != expected || form != ProjectedModelForm::Complete {
        return Err(py_type_error("projected key uses the wrong attribute wrapper"));
    }
    let scalar = wrapper.call_method0("runtime_attribute_value")?;
    Ok(Some((
        key.attr_name.clone(),
        attribute_value_from_py(py, &scalar, key.value_type)?,
    )))
}

fn projected_iid(value: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    let iid = value.getattr("iid")?;
    if iid.is_none() {
        Ok(None)
    } else {
        iid.extract().map(Some)
    }
}

fn normalized_items<'py>(
    value: Option<&Bound<'py, PyAny>>,
    cardinality: (u32, Option<u32>),
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let (minimum, maximum) = cardinality;
    let mut items = Vec::new();
    match value {
        None => {}
        Some(value) if value.is_none() => {}
        Some(value) if maximum == Some(1) => items.push(value.clone()),
        Some(value) => {
            if value.downcast::<PyString>().is_ok() {
                return Err(py_type_error("projected multi-value input requires a sequence"));
            }
            let tuple = value
                .downcast::<PyTuple>()
                .map_err(|_| py_type_error("projected multi-value input requires a tuple"))?;
            items.extend(tuple.iter());
        }
    }
    let count = u32::try_from(items.len()).map_err(|_| py_value_error("projected value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_value_error("projected value violates resolved cardinality"));
    }
    Ok(items)
}

fn descriptor_cardinality(descriptor: &OwnedAttributeDescriptor) -> (u32, Option<u32>) {
    descriptor
        .cardinality()
        .unwrap_or((u32::from(!descriptor.is_optional), Some(1)))
}

fn role_cardinality(descriptor: &RoleDescriptor) -> (u32, Option<u32>) {
    descriptor.cardinality.unwrap_or((0, Some(1)))
}

fn hydrate_entity(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicEntityRow,
) -> PyResult<PyObject> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .entity_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_attributes(py, package, &descriptor.owned_attributes, &row.attributes)?;
    hydrate_complete(py, package, id, &values, row.iid.as_deref())
}

fn hydrate_relation(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    row: &DynamicRelationRow,
) -> PyResult<PyObject> {
    ensure_row_type(id, row.type_name.as_deref())?;
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_attributes(py, package, &descriptor.owned_attributes, &row.attributes)?;
    let projection = package.projection.projection();
    let model = &projection.models()[id];
    for read in model.complete_read().roles().values() {
        let token = &model.query_tokens().roles()[read.role()];
        let role_name = read.role().label().as_str();
        let role = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("read role has no provider descriptor"))?;
        let mut players = Vec::new();
        for player in row.role_players.iter().filter(|player| player.role_name == role_name) {
            players.push(hydrate_player(py, package, read.players(), player)?);
        }
        set_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            players,
            role_cardinality(role),
        )?;
    }
    hydrate_complete(py, package, id, &values, row.iid.as_deref())
}

fn hydrate_player(
    py: Python<'_>,
    package: &InstalledPackage,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &DynamicRolePlayer,
) -> PyResult<PyObject> {
    let label = player
        .player_type_name
        .as_deref()
        .ok_or_else(|| py_runtime_error("role-player row has no concrete type label"))?;
    let id = package
        .types_by_label
        .get(label)
        .ok_or_else(|| py_runtime_error("role-player row type is outside the projection"))?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| py_runtime_error("role-player row type is not accepted by the projected role"))?;
    let attributes = package
        .projection
        .role_player_attributes(id, &player.attributes)
        .map_err(py_orm_error)?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            if id.kind() != TypeKind::Entity {
                return Err(py_runtime_error(
                    "nested complete relation hydration is forbidden; use its reference projection",
                ));
            }
            let row = DynamicEntityRow {
                iid: player.player_iid.clone(),
                type_name: player.player_type_name.clone(),
                attributes,
            };
            hydrate_entity(py, package, id, &row)
        }
        ProjectedModelForm::Reference => {
            let iid = player
                .player_iid
                .as_deref()
                .ok_or_else(|| py_runtime_error("reference role-player row has no IID"))?;
            let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
                TypeDescriptor::Entity(descriptor) => descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .cloned()
                    .collect::<Vec<_>>(),
                TypeDescriptor::Relation(_) => Vec::new(),
            };
            let values = hydrate_attributes(py, package, &descriptors, &attributes)?;
            hydrate_reference(py, package, id, &values, iid)
        }
    }
}

fn hydrate_attributes<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &DynamicAttributeMap,
) -> PyResult<Bound<'py, PyDict>> {
    let values = PyDict::new(py);
    for descriptor in descriptors {
        let mut wrappers = Vec::new();
        for (_, value) in attributes.iter().filter(|(name, _)| name == &descriptor.attr_name) {
            wrappers.push(hydrate_attribute(py, package, descriptor, value)?);
        }
        set_hydrated_values(
            py,
            &values,
            &descriptor.field_name,
            wrappers,
            descriptor_cardinality(descriptor),
        )?;
    }
    Ok(values)
}

fn hydrate_attribute(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    value: &AttributeValue,
) -> PyResult<PyObject> {
    ensure_attribute_type(value, descriptor.value_type)?;
    let id = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
    let class = package.class(id, ProjectedModelForm::Complete)?;
    let scalar = attribute_value_to_py(py, value)?;
    class.bind(py).call1((scalar,)).map(Bound::unbind)
}

fn set_hydrated_values(
    py: Python<'_>,
    values: &Bound<'_, PyDict>,
    name: &str,
    items: Vec<PyObject>,
    cardinality: (u32, Option<u32>),
) -> PyResult<()> {
    let (minimum, maximum) = cardinality;
    let count = u32::try_from(items.len()).map_err(|_| py_value_error("hydrated value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_runtime_error("provider row violates projected cardinality"));
    }
    if maximum == Some(1) {
        match items.into_iter().next() {
            Some(value) => values.set_item(name, value)?,
            None => values.set_item(name, py.None())?,
        }
    } else {
        values.set_item(name, PyTuple::new(py, items)?)?;
    }
    Ok(())
}

fn hydrate_complete(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: Option<&str>,
) -> PyResult<PyObject> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Complete)?)?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    if let Some(iid) = iid {
        instance.call_method1("attach_runtime_iid", (iid,))?;
    }
    Ok(instance.unbind())
}

fn hydrate_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
) -> PyResult<PyObject> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Reference)?)?;
    instance.call_method1("initialize_runtime_reference", (iid, values))?;
    Ok(instance.unbind())
}

fn allocate<'py>(py: Python<'py>, class: &Py<PyType>) -> PyResult<Bound<'py, PyAny>> {
    let class = class.bind(py);
    class.getattr("__new__")?.call1((class,))
}

fn ensure_row_type(id: &TypeId, actual: Option<&str>) -> PyResult<()> {
    if actual.is_some_and(|actual| actual != id.label().as_str()) {
        return Err(py_runtime_error("exact provider row returned a different concrete type"));
    }
    Ok(())
}

fn attribute_value_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
) -> PyResult<AttributeValue> {
    match value_type {
        ValueType::String => value
            .downcast_exact::<PyString>()
            .map_err(|_| py_type_error("attribute value requires an exact str"))?
            .extract()
            .map(AttributeValue::String),
        ValueType::Long => value
            .downcast_exact::<PyInt>()
            .map_err(|_| py_type_error("attribute value requires an exact int"))?
            .extract()
            .map(AttributeValue::Long),
        ValueType::Double => value
            .downcast_exact::<PyFloat>()
            .map_err(|_| py_type_error("attribute value requires an exact float"))?
            .extract()
            .map(AttributeValue::Double),
        ValueType::Boolean => value
            .downcast_exact::<PyBool>()
            .map_err(|_| py_type_error("attribute value requires an exact bool"))?
            .extract()
            .map(AttributeValue::Boolean),
        ValueType::Date => exact_temporal_string(py, value, "date", false).map(AttributeValue::Date),
        ValueType::DateTime => exact_temporal_string(py, value, "datetime", false).map(AttributeValue::DateTime),
        ValueType::DateTimeTz => exact_temporal_string(py, value, "datetime", true).map(AttributeValue::DateTimeTZ),
        ValueType::Decimal => exact_module_value_string(py, value, "decimal", "Decimal").map(AttributeValue::Decimal),
        ValueType::Duration => duration_from_py(py, value).map(AttributeValue::Duration),
    }
}

fn exact_temporal_string(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    class_name: &str,
    timezone_required: bool,
) -> PyResult<String> {
    let class = py.import("datetime")?.getattr(class_name)?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error(format!("attribute value requires an exact {class_name}")));
    }
    if class_name == "datetime" {
        let offset = value.call_method0("utcoffset")?;
        if timezone_required == offset.is_none() {
            return Err(py_type_error(if timezone_required {
                "datetime-tz requires a timezone-aware datetime"
            } else {
                "datetime requires a timezone-naive datetime"
            }));
        }
    }
    value.call_method0("isoformat")?.extract()
}

fn exact_module_value_string(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    module: &str,
    class_name: &str,
) -> PyResult<String> {
    let class = py.import(module)?.getattr(class_name)?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error(format!("attribute value requires an exact {class_name}")));
    }
    value.str()?.extract()
}

fn duration_from_py(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let class = py.import("datetime")?.getattr("timedelta")?;
    if value.get_type().as_ptr() != class.as_ptr() {
        return Err(py_type_error("attribute value requires an exact timedelta"));
    }
    let days: i64 = value.getattr("days")?.extract()?;
    let seconds: i64 = value.getattr("seconds")?.extract()?;
    let micros: i64 = value.getattr("microseconds")?.extract()?;
    if days < 0 {
        return Err(py_value_error("negative projected durations are not representable losslessly"));
    }
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    let fraction = if micros == 0 { String::new() } else { format!(".{micros:06}").trim_end_matches('0').to_owned() };
    Ok(format!("P{days}DT{hours}H{minutes}M{seconds}{fraction}S"))
}

fn ensure_attribute_type(value: &AttributeValue, expected: ValueType) -> PyResult<()> {
    let matches = matches!(
        (value, expected),
        (AttributeValue::String(_), ValueType::String)
            | (AttributeValue::Long(_), ValueType::Long)
            | (AttributeValue::Double(_), ValueType::Double)
            | (AttributeValue::Boolean(_), ValueType::Boolean)
            | (AttributeValue::Date(_), ValueType::Date)
            | (AttributeValue::DateTime(_), ValueType::DateTime)
            | (AttributeValue::DateTimeTZ(_), ValueType::DateTimeTz)
            | (AttributeValue::Decimal(_), ValueType::Decimal)
            | (AttributeValue::Duration(_), ValueType::Duration)
    );
    if matches { Ok(()) } else { Err(py_runtime_error("provider attribute value type disagrees with the projection")) }
}

fn attribute_value_to_py(py: Python<'_>, value: &AttributeValue) -> PyResult<PyObject> {
    match value {
        AttributeValue::String(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Long(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Double(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Boolean(value) => pythonize(py, value)
            .map(Bound::unbind)
            .map_err(|error| py_runtime_error(error.to_string())),
        AttributeValue::Date(value) => py.import("datetime")?.getattr("date")?.call_method1("fromisoformat", (value,)).map(Bound::unbind),
        AttributeValue::DateTime(value) | AttributeValue::DateTimeTZ(value) => py.import("datetime")?.getattr("datetime")?.call_method1("fromisoformat", (value,)).map(Bound::unbind),
        AttributeValue::Decimal(value) => py.import("decimal")?.getattr("Decimal")?.call1((value,)).map(Bound::unbind),
        AttributeValue::Duration(_) => Err(py_value_error("duration hydration requires a lossless day-time parser")),
    }
}

fn py_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> PyErr {
    py_value_error(error.to_string())
}

fn py_orm_error(error: type_bridge_orm::OrmError) -> PyErr {
    py_runtime_error(error.to_string())
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

fn py_type_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyTypeError::new_err(message.into())
}

fn py_runtime_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(message.into())
}

/// Register verified runtime projection classes on the Python extension module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRuntimeProjection>()?;
    module.add_class::<PyProjectedModelManager>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use pyo3::ffi;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use super::*;

    const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  aliases: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      aliases: { card: { min: 0, max: 2 } }
relations:
  membership:
    relates:
      member: { card: 1 }
  event: {}
  container:
    relates:
      item: { card: { min: 0, max: 2 } }
plays:
  person:
    membership: [member]
  event:
    container: [item]
"#;

    fn projection() -> RuntimeProjection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("python-native.yaml").unwrap(),
            SCHEMA,
        )]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        ).unwrap()
    }

    fn classes(py: Python<'_>, projection: &RuntimeProjection) -> Vec<(Py<PyType>, Option<Py<PyType>>)> {
        let module = PyModule::from_code(
            py,
            ffi::c_str!(r#"
class Complete:
    __model_form__ = "complete"
    def __init__(self, **values):
        self._values = dict(values)
        self._iid = None
    @property
    def iid(self):
        return self._iid
    def runtime_values(self):
        return self._values
    def initialize_runtime_values(self, values):
        self._values = dict(values)
        self._iid = None
    def attach_runtime_iid(self, iid):
        self._iid = iid

class Attribute(Complete):
    def __init__(self, value):
        super().__init__()
        self._attribute_value = value
    def runtime_attribute_value(self):
        return self._attribute_value

class Reference:
    __model_form__ = "reference"
    def __init__(self, iid, **values):
        self.initialize_runtime_reference(iid, values)
    @property
    def iid(self):
        return self._iid
    def runtime_values(self):
        return self._values
    def initialize_runtime_reference(self, iid, values):
        self._iid = iid
        self._values = dict(values)
"#),
            ffi::c_str!("projection_models.py"),
            ffi::c_str!("projection_models"),
        ).unwrap();
        let builtins = py.import("builtins").unwrap();
        let type_fn = builtins.getattr("type").unwrap();
        projection.models().iter().map(|(id, model)| {
            let base = if id.kind() == TypeKind::Attribute { "Attribute" } else { "Complete" };
            let attrs = PyDict::new(py);
            attrs.set_item("__type_id__", canonical_id(id).unwrap()).unwrap();
            attrs.set_item("__model_form__", "complete").unwrap();
            let bases = PyTuple::new(py, [module.getattr(base).unwrap()]).unwrap();
            let complete = type_fn.call1((model.target_name().as_str(), bases, attrs)).unwrap().downcast_into::<PyType>().unwrap().unbind();
            let reference = model.reference_read().target_name().map(|name| {
                let attrs = PyDict::new(py);
                attrs.set_item("__type_id__", canonical_id(id).unwrap()).unwrap();
                attrs.set_item("__model_form__", "reference").unwrap();
                let bases = PyTuple::new(py, [module.getattr("Reference").unwrap()]).unwrap();
                type_fn.call1((name.as_str(), bases, attrs)).unwrap().downcast_into::<PyType>().unwrap().unbind()
            });
            (complete, reference)
        }).collect()
    }

    fn install(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let projection = projection();
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic = String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap()).unwrap();
        let fingerprint = String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap()).unwrap();
        let package = install_projection(py, &projection_json, &semantic, &fingerprint, classes(py, &projection)).unwrap();
        (projection, package)
    }

    #[test]
    fn install_is_canonical_tamper_evident_and_requires_exact_coverage() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let projection = projection();
            let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic = String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap()).unwrap();
            let fingerprint = String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap()).unwrap();
            install_projection(py, &projection_json, &semantic, &fingerprint, classes(py, &projection)).unwrap();

            let mut missing = classes(py, &projection);
            missing.pop();
            assert!(install_projection(py, &projection_json, &semantic, &fingerprint, missing).is_err());

            let mut tampered: serde_json::Value = serde_json::from_str(&projection_json).unwrap();
            tampered["models"][0]["target_name"] = serde_json::json!("Tampered");
            let tampered = String::from_utf8(to_canonical_json(&tampered).unwrap()).unwrap();
            assert!(install_projection(py, &tampered, &semantic, &fingerprint, classes(py, &projection)).is_err());
        });
    }

    #[test]
    fn native_lowering_and_hydration_preserve_wrappers_iids_and_relation_references() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install(py);
            let person_id = package.type_by_label("person", TypeKind::Entity).unwrap().clone();
            let identifier_id = package.type_by_label("identifier", TypeKind::Attribute).unwrap().clone();
            let aliases_id = package.type_by_label("aliases", TypeKind::Attribute).unwrap().clone();
            let person_class = package.class(&person_id, ProjectedModelForm::Complete).unwrap().bind(py);
            let identifier_class = package.class(&identifier_id, ProjectedModelForm::Complete).unwrap().bind(py);
            let aliases_class = package.class(&aliases_id, ProjectedModelForm::Complete).unwrap().bind(py);
            let identifier = identifier_class.call1(("person-1",)).unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("identifier", &identifier).unwrap();
            let person = person_class.call((), Some(&kwargs)).unwrap();
            let descriptor = package.projection.entity_descriptor(&person_id).unwrap();
            assert_eq!(
                lower_attributes(py, package.as_ref(), &descriptor.owned_attributes, &person).unwrap(),
                vec![("identifier".into(), AttributeValue::String("person-1".into()))]
            );

            let hydrated = hydrate_entity(py, package.as_ref(), &person_id, &DynamicEntityRow {
                iid: Some("0x-person".into()),
                type_name: Some("person".into()),
                attributes: vec![("identifier".into(), AttributeValue::String("person-1".into()))],
            }).unwrap();
            let hydrated = hydrated.bind(py);
            assert_eq!(hydrated.getattr("iid").unwrap().extract::<String>().unwrap(), "0x-person");
            let wrapped = hydrated.call_method0("runtime_values").unwrap().downcast::<PyDict>().unwrap().get_item("identifier").unwrap().unwrap();
            assert_eq!(wrapped.get_type().as_ptr(), identifier_class.as_ptr());
            assert_eq!(wrapped.call_method0("runtime_attribute_value").unwrap().extract::<String>().unwrap(), "person-1");

            let membership_id = package.type_by_label("membership", TypeKind::Relation).unwrap().clone();
            let membership = hydrate_relation(py, package.as_ref(), &membership_id, &DynamicRelationRow {
                iid: Some("0x-membership".into()),
                type_name: Some("membership".into()),
                attributes: vec![],
                role_players: vec![DynamicRolePlayer {
                    role_name: "member".into(),
                    player_iid: Some("0x-person".into()),
                    player_type_name: Some("person".into()),
                    attributes: vec![
                        ("identifier".into(), serde_json::json!("person-1")),
                        ("aliases".into(), serde_json::json!(["alpha", "beta"])),
                    ],
                }],
            }).unwrap();
            let membership_values = membership.bind(py).call_method0("runtime_values").unwrap();
            let member = membership_values.downcast::<PyDict>().unwrap().get_item("member").unwrap().unwrap();
            assert_eq!(member.get_type().as_ptr(), person_class.as_ptr());
            assert_eq!(member.getattr("iid").unwrap().extract::<String>().unwrap(), "0x-person");
            let member_values = member.call_method0("runtime_values").unwrap();
            let member_values = member_values.downcast::<PyDict>().unwrap();
            let member_identifier = member_values.get_item("identifier").unwrap().unwrap();
            assert_eq!(member_identifier.get_type().as_ptr(), identifier_class.as_ptr());
            assert_eq!(member_identifier.call_method0("runtime_attribute_value").unwrap().extract::<String>().unwrap(), "person-1");
            let member_aliases = member_values.get_item("aliases").unwrap().unwrap();
            let member_aliases = member_aliases.downcast::<PyTuple>().unwrap();
            assert_eq!(member_aliases.len(), 2);
            for alias in member_aliases.iter() {
                assert_eq!(alias.get_type().as_ptr(), aliases_class.as_ptr());
            }

            let container_id = package.type_by_label("container", TypeKind::Relation).unwrap().clone();
            let relation = hydrate_relation(py, package.as_ref(), &container_id, &DynamicRelationRow {
                iid: Some("0x-container".into()),
                type_name: Some("container".into()),
                attributes: vec![],
                role_players: vec![DynamicRolePlayer {
                    role_name: "item".into(),
                    player_iid: Some("0x-event".into()),
                    player_type_name: Some("event".into()),
                    attributes: vec![],
                }],
            }).unwrap();
            let values = relation.bind(py).call_method0("runtime_values").unwrap();
            let item = values.downcast::<PyDict>().unwrap().get_item("item").unwrap().unwrap();
            let item = item.downcast::<PyTuple>().unwrap().get_item(0).unwrap();
            assert_eq!(item.getattr("__model_form__").unwrap().extract::<String>().unwrap(), "reference");
            assert_eq!(item.getattr("iid").unwrap().extract::<String>().unwrap(), "0x-event");
        });
    }
}
