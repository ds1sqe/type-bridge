//! Verified package-scoped runtime projections for generated Python models.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType};
use pythonize::pythonize;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::ProjectedModelForm;
#[cfg(test)]
use type_bridge_contract::projection::RuntimeProjection;
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_core_lib::ast::{Clause, Constraint, Pattern, RolePlayer, Statement};
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use type_bridge_orm::_dynamic::{
    DynamicAttributeMap, DynamicComparisonOp, DynamicEntityRow, DynamicExpr, DynamicRelationRow,
    DynamicRolePlayer, DynamicRolePlayerInput,
};
use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::session::{Database, TransactionContext};
use type_bridge_orm::value::AttributeValue;
use type_bridge_orm::{HydratedAttribute, HydratedRolePlayer, HydratedThing, ThingKind};
use type_bridge_orm::{InstalledRuntimeProjection, ProviderRuntimeOwner};

use crate::match_runtime::PyMatchSessionHandle;
use crate::orm_runtime::{PyRustDatabase, PyRustTransactionContext, provider_block_on};
use crate::validated_result_runtime::PyValidatedMatchThingHandle;

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
    fn model_id_for_class(&self, py: Python<'_>, class: &Py<PyType>) -> PyResult<TypeId> {
        let pointer = class.bind(py).as_ptr();
        self.models
            .iter()
            .find_map(|(id, registered)| {
                (registered.complete.bind(py).as_ptr() == pointer).then(|| id.clone())
            })
            .ok_or_else(|| {
                py_type_error("model class is not registered in this runtime projection")
            })
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

    fn class(&self, id: &TypeId, form: ProjectedModelForm) -> PyResult<&Py<PyType>> {
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
            filters: vec![],
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
            filters: vec![],
        })
    }

    /// Validate one exact generated attribute scalar through the Rust projection contract.
    fn validate_attribute_value(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        value: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Attribute {
            return Err(py_type_error(
                "generated scalar validation requires an exact attribute class",
            ));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| py_runtime_error("projection attribute model is absent"))?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| py_runtime_error("projection attribute has no scalar domain"))?;
        let value = attribute_value_from_py(py, &value, projected_value_type(value_type))?;
        self.package
            .projection
            .validate_attribute_value(&id, &value)
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Validate one exact generated owned-field value through the Rust projection contract.
    fn validate_field_value(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        field_name: &str,
        value: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        let projected = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| py_runtime_error("projection model is absent"))?;
        let field = projected
            .query_tokens()
            .fields()
            .values()
            .find(|field| field.target_name().as_str() == field_name)
            .ok_or_else(|| {
                py_value_error("generated value references an unknown projected field")
            })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                .map_err(py_diagnostic)?;
        let (actual_id, form) = self.package.identify_value(py, &value)?;
        if actual_id != attribute_id || form != ProjectedModelForm::Complete {
            return Err(py_type_error(
                "generated owned field requires its exact attribute wrapper",
            ));
        }
        let attribute = self
            .package
            .projection
            .projection()
            .models()
            .get(&attribute_id)
            .ok_or_else(|| py_runtime_error("projection field attribute is absent"))?;
        let value_type = attribute
            .declaration()
            .value_type()
            .ok_or_else(|| py_runtime_error("projection field attribute has no scalar domain"))?;
        let scalar = value.call_method0("runtime_attribute_value")?;
        let scalar = attribute_value_from_py(py, &scalar, projected_value_type(value_type))?;
        self.package
            .projection
            .validate_field_value(&id, field_name, &scalar)
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Compile a retained raw-query entity match from one exact generated class.
    fn query_builder_match_entity(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        variable: &str,
        filters: &Bound<'_, PyDict>,
    ) -> PyResult<String> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Entity {
            return Err(py_type_error(
                "generated entity matches require an exact installed entity class",
            ));
        }
        let descriptor = self
            .package
            .projection
            .entity_descriptor(&id)
            .map_err(py_orm_error)?;
        let mut constraints = Vec::with_capacity(filters.len());
        for (field_name, value) in filters.iter() {
            let field_name: String = field_name
                .downcast_exact::<PyString>()
                .map_err(|_| py_type_error("generated entity filter names must be exact strings"))?
                .extract()?;
            let field = descriptor
                .owned_attributes
                .iter()
                .find(|field| field.field_name == field_name)
                .ok_or_else(|| {
                    py_value_error(format!(
                        "generated entity {:?} has no projected field {field_name:?}",
                        id.label().as_str()
                    ))
                })?;
            let attribute_id = self
                .package
                .type_by_label(&field.attr_name, TypeKind::Attribute)?;
            let attribute_class = self
                .package
                .class(attribute_id, ProjectedModelForm::Complete)?;
            let scalar = if value.get_type().as_ptr() == attribute_class.bind(py).as_ptr() {
                value.call_method0("runtime_attribute_value")?
            } else {
                value
            };
            let value = attribute_value_from_py(py, &scalar, field.value_type)?;
            constraints.push(Constraint::Has {
                attr_name: field.attr_name.clone(),
                value: value.to_ast_value(),
            });
        }
        compatibility_clause_body(
            Clause::Match(vec![Pattern::Entity {
                variable: variable.to_owned(),
                type_name: id.label().as_str().to_owned(),
                constraints,
                is_strict: false,
            }]),
            "match",
        )
    }

    /// Compile a retained raw-query entity insert from one exact generated value.
    fn query_builder_insert_entity(
        &self,
        py: Python<'_>,
        instance: Bound<'_, PyAny>,
        variable: &str,
    ) -> PyResult<String> {
        let (id, form) = self.package.identify_value(py, &instance)?;
        if id.kind() != TypeKind::Entity || form != ProjectedModelForm::Complete {
            return Err(py_type_error(
                "generated entity inserts require an exact installed complete entity",
            ));
        }
        let descriptor = self
            .package
            .projection
            .entity_descriptor(&id)
            .map_err(py_orm_error)?;
        let attributes = lower_attributes(
            py,
            self.package.as_ref(),
            &descriptor.owned_attributes,
            &instance,
        )?;
        let mut statements = vec![Statement::Isa {
            variable: variable.to_owned(),
            type_name: id.label().as_str().to_owned(),
        }];
        statements.extend(
            attributes
                .into_iter()
                .map(|(attribute, value)| Statement::Has {
                    subject_var: variable.to_owned(),
                    attr_name: attribute,
                    value: value.to_ast_value(),
                }),
        );
        compatibility_clause_body(Clause::Insert(statements), "insert")
    }

    /// Compile a retained raw-query relation match from generated role tokens.
    #[pyo3(signature = (model, variable, role_players=None))]
    fn query_builder_match_relation(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        variable: &str,
        role_players: Option<Bound<'_, PyDict>>,
    ) -> PyResult<String> {
        let id = self.package.model_id_for_class(py, &model)?;
        if id.kind() != TypeKind::Relation {
            return Err(py_type_error(
                "generated relation matches require an exact installed relation class",
            ));
        }
        let projected = &self.package.projection.projection().models()[&id];
        let mut players = Vec::new();
        if let Some(role_players) = role_players {
            players.reserve(role_players.len());
            for (role_name, player_variable) in role_players.iter() {
                let role_name: String = role_name
                    .downcast_exact::<PyString>()
                    .map_err(|_| {
                        py_type_error("generated relation role names must be exact strings")
                    })?
                    .extract()?;
                let token = projected
                    .query_tokens()
                    .roles()
                    .values()
                    .find(|token| token.target_name().as_str() == role_name)
                    .ok_or_else(|| {
                        py_value_error(format!(
                            "generated relation {:?} has no projected role {role_name:?}",
                            id.label().as_str()
                        ))
                    })?;
                let player_variable: String = player_variable
                    .downcast_exact::<PyString>()
                    .map_err(|_| {
                        py_type_error("generated relation player variables must be exact strings")
                    })?
                    .extract()?;
                players.push(RolePlayer {
                    role: token.role().label().as_str().to_owned(),
                    player_var: player_variable,
                });
            }
        }
        compatibility_clause_body(
            Clause::Match(vec![Pattern::Relation {
                variable: variable.to_owned(),
                type_name: id.label().as_str().to_owned(),
                role_players: players,
                constraints: Vec::new(),
            }]),
            "match",
        )
    }

    /// Build an opaque match session from this exact installed projection only.
    fn match_session(&self) -> PyResult<PyMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(py_orm_error)?;
        Ok(PyMatchSessionHandle::from_registry(Arc::new(registry)))
    }

    /// Hydrate one proof-backed query result through this exact package projection.
    fn hydrate_thing(
        &self,
        py: Python<'_>,
        thing: PyRef<'_, PyValidatedMatchThingHandle>,
    ) -> PyResult<PyObject> {
        hydrate_validated_thing(py, self.package.as_ref(), &thing)
    }
}

/// Exact CRUD manager for one generated projected entity or relation class.
#[pyclass]
pub struct PyProjectedModelManager {
    package: Arc<InstalledPackage>,
    type_id: TypeId,
    database: Option<Arc<Database>>,
    transaction: Option<TransactionContext>,
    runtime: Arc<ProviderRuntimeOwner>,
    filters: Vec<DynamicExpr>,
}

#[pymethods]
impl PyProjectedModelManager {
    /// Insert one exact generated model and attach the returned TypeDB IID.
    fn insert(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<PyObject> {
        self.ensure_instance(py, &instance)?;
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.insert(&attributes))
                    .map_err(py_orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.insert(&attributes, &players),
                )
                .map_err(py_orm_error)?
            }
        };
        instance.call_method1("attach_runtime_iid", (iid,))?;
        Ok(instance.unbind())
    }

    /// Insert exact generated models atomically and attach IIDs in input order.
    fn insert_many(&self, py: Python<'_>, instances: Vec<PyObject>) -> PyResult<PyObject> {
        self.write_many(py, instances, false)
    }

    /// Insert or update one exact generated model and attach its TypeDB IID.
    fn put(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<PyObject> {
        self.ensure_instance(py, &instance)?;
        let iid = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.put_exact(&attributes))
                    .map_err(py_orm_error)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.put_exact(&attributes, &players),
                )
                .map_err(py_orm_error)?
            }
        };
        instance.call_method1("attach_runtime_iid", (iid,))?;
        Ok(instance.unbind())
    }

    /// Put exact generated models atomically and attach IIDs in input order.
    fn put_many(&self, py: Python<'_>, instances: Vec<PyObject>) -> PyResult<PyObject> {
        self.write_many(py, instances, true)
    }

    /// Replace one exact generated model already identified by its TypeDB IID.
    fn update(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<PyObject> {
        self.ensure_instance(py, &instance)?;
        let iid = required_projected_iid(&instance)?;
        let hydrated = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_and_get_exact(&iid, &attributes),
                )
                .map_err(py_orm_error)?;
                hydrate_entity(py, self.package.as_ref(), &self.type_id, &row)?
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let players = lower_roles(
                    py,
                    self.package.as_ref(),
                    &self.type_id,
                    &descriptor,
                    &instance,
                )?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_and_get_exact(&iid, &attributes, &players),
                )
                .map_err(py_orm_error)?;
                hydrate_relation(py, self.package.as_ref(), &self.type_id, &row)?
            }
        };
        replace_projected_instance(py, instance, hydrated)
    }

    /// Replace exact generated models atomically and rehydrate them in input order.
    fn update_many(&self, py: Python<'_>, instances: Vec<PyObject>) -> PyResult<PyObject> {
        if instances.is_empty() {
            return Ok(PyList::empty(py).into_any().unbind());
        }
        for instance in &instances {
            self.ensure_instance(py, instance.bind(py))?;
        }
        let hydrated = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            required_projected_iid(instance.bind(py))?,
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_many_and_get_exact(&items),
                )
                .map_err(py_orm_error)?
                .iter()
                .map(|row| hydrate_entity(py, self.package.as_ref(), &self.type_id, row))
                .collect::<PyResult<Vec<_>>>()?
            }
            TypeDescriptor::Relation(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            required_projected_iid(instance.bind(py))?,
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                            lower_roles(
                                py,
                                self.package.as_ref(),
                                &self.type_id,
                                &descriptor,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.update_many_and_get_exact(&items),
                )
                .map_err(py_orm_error)?
                .iter()
                .map(|row| hydrate_relation(py, self.package.as_ref(), &self.type_id, row))
                .collect::<PyResult<Vec<_>>>()?
            }
        };
        if hydrated.len() != instances.len() {
            return Err(py_runtime_error(
                "projected batch update returned an unexpected model count",
            ));
        }
        for (instance, stored) in instances.iter().zip(hydrated) {
            replace_projected_instance(py, instance.bind(py).clone(), stored)?;
        }
        Ok(PyList::new(py, &instances)?.into_any().unbind())
    }

    /// Resolve an IID-less exact generated model through its projected identity.
    ///
    /// Entities use every declared key. Relations use every populated owned
    /// attribute and role player, matching the detached-instance behavior of
    /// the pre-cutover manager without accepting handwritten descriptors.
    fn resolve_iid(&self, py: Python<'_>, instance: Bound<'_, PyAny>) -> PyResult<Option<String>> {
        self.ensure_instance(py, &instance)?;
        if let Some(iid) = projected_iid(&instance)? {
            return Ok(Some(iid));
        }
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let keys = descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .collect::<Vec<_>>();
                if keys.is_empty() {
                    return Err(py_value_error(format!(
                        "generated entity {:?} requires an attached IID or projected key",
                        descriptor.type_name
                    )));
                }
                let mut expressions = Vec::with_capacity(keys.len());
                for key in keys {
                    let value = attributes
                        .iter()
                        .find_map(|(name, value)| (name == &key.attr_name).then(|| value.clone()))
                        .ok_or_else(|| {
                            py_value_error(format!(
                                "generated entity {:?} requires projected key {:?}",
                                descriptor.type_name, key.field_name
                            ))
                        })?;
                    expressions.push(DynamicExpr::Compare {
                        attr_name: key.attr_name.clone(),
                        operator: DynamicComparisonOp::Eq,
                        value,
                    });
                }
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&expressions, &[], Some(1), None),
                )
                .map_err(py_orm_error)?;
                resolved_entity_iid(rows.first())
            }
            TypeDescriptor::Relation(descriptor) => {
                let attributes = lower_attributes(
                    py,
                    self.package.as_ref(),
                    &descriptor.owned_attributes,
                    &instance,
                )?;
                let key_names = descriptor
                    .owned_attributes
                    .iter()
                    .filter(|attribute| attribute.is_key())
                    .map(|attribute| attribute.attr_name.clone())
                    .collect::<BTreeSet<_>>();
                let mut expressions = attributes
                    .into_iter()
                    .filter(|(attr_name, _)| key_names.is_empty() || key_names.contains(attr_name))
                    .map(|(attr_name, value)| DynamicExpr::Compare {
                        attr_name,
                        operator: DynamicComparisonOp::Eq,
                        value,
                    })
                    .collect::<Vec<_>>();
                if key_names.is_empty() {
                    let players = lower_roles(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &descriptor,
                        &instance,
                    )?;
                    expressions.extend(players.into_iter().map(|player| {
                        let expr = match (player.iid, player.key) {
                            (Some(iid), None) => DynamicExpr::Iid { iid },
                            (None, Some((attr_name, value))) => DynamicExpr::Compare {
                                attr_name,
                                operator: DynamicComparisonOp::Eq,
                                value,
                            },
                            _ => unreachable!("lower_roles enforces exactly one player identity"),
                        };
                        DynamicExpr::RolePlayer {
                            role_name: player.role_name,
                            expr: Box::new(expr),
                        }
                    }));
                }
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&expressions, &[], Some(1), None),
                )
                .map_err(py_orm_error)?;
                resolved_relation_iid(rows.first())
            }
        }
    }

    /// Delete one exact generated model by its instance or canonical TypeDB IID.
    fn delete(&self, py: Python<'_>, instance_or_iid: Bound<'_, PyAny>) -> PyResult<()> {
        let iid = if let Ok(iid) = instance_or_iid.downcast_exact::<PyString>() {
            iid.to_str()?.to_owned()
        } else {
            self.ensure_instance(py, &instance_or_iid)?;
            required_projected_iid(&instance_or_iid)?
        };
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.delete_by_iid_exact(&iid))
                    .map_err(py_orm_error)?;
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(py, self.runtime.as_ref(), manager.delete_by_iid_exact(&iid))
                    .map_err(py_orm_error)?;
            }
        }
        Ok(())
    }

    /// Delete exact generated models atomically by canonical IID.
    fn delete_many(&self, py: Python<'_>, iids: Vec<String>) -> PyResult<()> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.delete_many_by_iid_exact(&iids),
                )
                .map_err(py_orm_error)?;
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.delete_many_by_iid_exact(&iids),
                )
                .map_err(py_orm_error)?;
            }
        }
        Ok(())
    }

    /// Return a new exact generated-model manager narrowed by keyword filters.
    #[pyo3(signature = (**filters))]
    fn filter(&self, py: Python<'_>, filters: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let descriptor = self.descriptor()?;
        let attributes = match &descriptor {
            TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
            TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
        };
        let mut combined = self.filters.clone();
        combined.extend(lower_filter_kwargs(
            py,
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

    /// Fetch all exact instances of this projected type using `isa!`.
    fn all(&self, py: Python<'_>) -> PyResult<PyObject> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&self.filters, &[], None, None),
                )
                .map_err(py_orm_error)?;
                let values = PyList::empty(py);
                for row in rows {
                    values.append(hydrate_entity(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &row,
                    )?)?;
                }
                Ok(values.into_any().unbind())
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.get_exact_with_query(&self.filters, &[], None, None),
                )
                .map_err(py_orm_error)?;
                let values = PyList::empty(py);
                for row in rows {
                    values.append(hydrate_relation(
                        py,
                        self.package.as_ref(),
                        &self.type_id,
                        &row,
                    )?)?;
                }
                Ok(values.into_any().unbind())
            }
        }
    }

    /// Return the first exact filtered model, or `None` when no model matches.
    fn first(&self, py: Python<'_>) -> PyResult<PyObject> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.first_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)?;
                row.as_ref()
                    .map(|row| hydrate_entity(py, self.package.as_ref(), &self.type_id, row))
                    .transpose()
                    .map(|value| value.unwrap_or_else(|| py.None()))
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let row = provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.first_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)?;
                row.as_ref()
                    .map(|row| hydrate_relation(py, self.package.as_ref(), &self.type_id, row))
                    .transpose()
                    .map(|value| value.unwrap_or_else(|| py.None()))
            }
        }
    }

    /// Count exact filtered models.
    fn count(&self, py: Python<'_>) -> PyResult<u64> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.count_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.count_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
        }
    }

    /// Return whether at least one exact filtered model exists.
    fn exists(&self, py: Python<'_>) -> PyResult<bool> {
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.exists_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                provider_block_on(
                    py,
                    self.runtime.as_ref(),
                    manager.exists_exact_with_query(&self.filters),
                )
                .map_err(py_orm_error)
            }
        }
    }

    /// Fetch one exact instance by TypeDB IID using `isa!`.
    fn get_by_iid(&self, py: Python<'_>, iid: &str) -> PyResult<PyObject> {
        // Preserve the released Python manager contract: malformed IIDs are
        // indistinguishable from absent IIDs for this convenience lookup.
        // Query predicates remain strict and reject malformed IIDs before I/O.
        if !is_canonical_thing_iid(iid) {
            return Ok(py.None());
        }
        let iid = iid.to_owned();
        match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let manager = self.entity_manager(Arc::new(descriptor))?;
                let row =
                    provider_block_on(py, self.runtime.as_ref(), manager.get_by_iid_exact(&iid))
                        .map_err(py_orm_error)?;
                match row {
                    Some(row) => hydrate_entity(py, self.package.as_ref(), &self.type_id, &row),
                    None => Ok(py.None()),
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let manager = self.relation_manager(Arc::new(descriptor))?;
                let rows =
                    provider_block_on(py, self.runtime.as_ref(), manager.get_by_iid_exact(&iid))
                        .map_err(py_orm_error)?;
                match rows.as_slice() {
                    [] => Ok(py.None()),
                    [row] => hydrate_relation(py, self.package.as_ref(), &self.type_id, row),
                    _ => Err(py_runtime_error(
                        "exact IID relation query returned multiple rows",
                    )),
                }
            }
        }
    }
}

impl PyProjectedModelManager {
    fn write_many(
        &self,
        py: Python<'_>,
        instances: Vec<PyObject>,
        put: bool,
    ) -> PyResult<PyObject> {
        if instances.is_empty() {
            return Ok(PyList::empty(py).into_any().unbind());
        }
        for instance in &instances {
            self.ensure_instance(py, instance.bind(py))?;
        }
        let iids = match self.descriptor()? {
            TypeDescriptor::Entity(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        lower_attributes(
                            py,
                            self.package.as_ref(),
                            &descriptor.owned_attributes,
                            instance.bind(py),
                        )
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.entity_manager(Arc::new(descriptor))?;
                if put {
                    provider_block_on(py, self.runtime.as_ref(), manager.put_many_exact(&items))
                        .map_err(py_orm_error)?
                } else {
                    provider_block_on(py, self.runtime.as_ref(), manager.insert_many(&items))
                        .map_err(py_orm_error)?
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                let items = instances
                    .iter()
                    .map(|instance| {
                        Ok((
                            lower_attributes(
                                py,
                                self.package.as_ref(),
                                &descriptor.owned_attributes,
                                instance.bind(py),
                            )?,
                            lower_roles(
                                py,
                                self.package.as_ref(),
                                &self.type_id,
                                &descriptor,
                                instance.bind(py),
                            )?,
                        ))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let manager = self.relation_manager(Arc::new(descriptor))?;
                if put {
                    provider_block_on(py, self.runtime.as_ref(), manager.put_many_exact(&items))
                        .map_err(py_orm_error)?
                } else {
                    provider_block_on(py, self.runtime.as_ref(), manager.insert_many(&items))
                        .map_err(py_orm_error)?
                }
            }
        };
        if iids.len() != instances.len() {
            return Err(py_runtime_error(
                "projected batch write returned an unexpected IID count",
            ));
        }
        for (instance, iid) in instances.iter().zip(iids) {
            instance
                .bind(py)
                .call_method1("attach_runtime_iid", (iid,))?;
        }
        Ok(PyList::new(py, &instances)?.into_any().unbind())
    }

    fn descriptor(&self) -> PyResult<TypeDescriptor> {
        self.package
            .projection
            .descriptor(&self.type_id)
            .cloned()
            .map_err(py_orm_error)
    }

    fn ensure_instance(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let expected = self
            .package
            .class(&self.type_id, ProjectedModelForm::Complete)?;
        if value.get_type().as_ptr() != expected.bind(py).as_ptr() {
            return Err(py_type_error(
                "insert requires an instance of the manager's exact registered class",
            ));
        }
        Ok(())
    }

    fn entity_manager(
        &self,
        descriptor: Arc<EntityDescriptor>,
    ) -> PyResult<DynamicEntityManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicEntityManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicEntityManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
    }

    fn relation_manager(
        &self,
        descriptor: Arc<RelationDescriptor>,
    ) -> PyResult<DynamicRelationManager<'_>> {
        if let Some(transaction) = &self.transaction {
            return Ok(DynamicRelationManager::with_canonical_transaction(
                transaction.clone(),
                descriptor,
            ));
        }
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| py_runtime_error("projected manager has no execution target"))?;
        Ok(DynamicRelationManager::new_canonical(
            database.as_ref(),
            descriptor,
        ))
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
        expected.insert(
            canonical_id(id)?,
            (
                id.clone(),
                model.target_name().as_str().to_owned(),
                model
                    .reference_read()
                    .target_name()
                    .map(|name| name.as_str().to_owned()),
            ),
        );
        if types_by_label
            .insert(id.label().as_str().to_owned(), id.clone())
            .is_some()
        {
            return Err(py_runtime_error(
                "projection contains duplicate type labels",
            ));
        }
    }
    if models.len() != expected.len() {
        return Err(py_value_error(format!(
            "projection requires exactly {} model registrations, received {}",
            expected.len(),
            models.len()
        )));
    }
    let mut registered = BTreeMap::new();
    let mut pointers = BTreeSet::new();
    for (complete, reference) in models {
        let id_text: String = complete.bind(py).getattr("__type_id__")?.extract()?;
        let (id, complete_name, reference_name) = expected.remove(&id_text).ok_or_else(|| {
            py_value_error("registered model has an unknown or duplicate __type_id__")
        })?;
        verify_class(py, &complete, &complete_name, "complete", &mut pointers)?;
        match (&reference_name, &reference) {
            (Some(expected_name), Some(class)) => {
                verify_class(py, class, expected_name, "reference", &mut pointers)?;
            }
            (None, None) => {}
            (Some(_), None) => return Err(py_value_error("projection reference class is missing")),
            (None, Some(_)) => return Err(py_value_error("unexpected projection reference class")),
        }
        registered.insert(
            id,
            RegisteredModel {
                complete,
                reference,
            },
        );
    }
    if !expected.is_empty() {
        return Err(py_value_error(
            "projection model registration coverage is incomplete",
        ));
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
        return Err(py_value_error(
            "one Python class was registered for multiple projected forms",
        ));
    }
    Ok(())
}

fn canonical_id(id: &TypeId) -> PyResult<String> {
    String::from_utf8(to_canonical_json(id).map_err(py_diagnostic)?)
        .map_err(|error| py_runtime_error(error.to_string()))
}

fn ensure_manageable(package: &InstalledPackage, id: &TypeId) -> PyResult<()> {
    if package.projection.descriptor(id).is_err() {
        return Err(py_type_error(
            "attribute projections do not expose CRUD managers",
        ));
    }
    Ok(())
}

fn compatibility_clause_body(clause: Clause, keyword: &str) -> PyResult<String> {
    let compiled = QueryCompiler::new().compile_clause(&clause);
    let prefix = format!("{keyword}\n");
    compiled
        .strip_prefix(&prefix)
        .and_then(|body| body.strip_suffix(';'))
        .map(str::to_owned)
        .ok_or_else(|| py_runtime_error("native query compiler returned an invalid clause shape"))
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

fn lower_filter_kwargs(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    filters: Option<&Bound<'_, PyDict>>,
) -> PyResult<Vec<DynamicExpr>> {
    let Some(filters) = filters else {
        return Ok(vec![]);
    };
    let mut lowered = Vec::with_capacity(filters.len());
    for (key, value) in filters {
        let key = key
            .downcast::<PyString>()
            .map_err(|_| py_type_error("generated manager filter names must be strings"))?
            .to_str()?;
        if matches!(key, "iid" | "_iid" | "iid__eq" | "_iid__eq") {
            lowered.push(DynamicExpr::Iid {
                iid: projected_filter_iid(&value)?,
            });
            continue;
        }
        if matches!(key, "iid__in" | "_iid__in") {
            let iids = projected_filter_items(&value, "iid__in")?
                .iter()
                .map(projected_filter_iid)
                .collect::<PyResult<Vec<_>>>()?;
            lowered.push(DynamicExpr::Or {
                exprs: iids
                    .into_iter()
                    .map(|iid| DynamicExpr::Iid { iid })
                    .collect(),
            });
            continue;
        }
        // A generated field may itself contain `__`. A recognised trailing
        // lookup wins only when the prefix is also a field, so `score__gte`
        // remains the comparison on `score`. Use `score__gte__eq` to select
        // equality on a field literally named `score__gte`.
        let parsed_lookup = key.rsplit_once("__");
        let has_field = |name: &str| {
            descriptors
                .iter()
                .any(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
        };
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
            _ if has_field(key) => (key, "eq"),
            Some((field_name, lookup)) => (field_name, lookup),
            None => (key, "eq"),
        };
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == field_name || descriptor.attr_name == field_name
            })
            .ok_or_else(|| {
                py_value_error(format!("unknown generated manager filter {field_name:?}"))
            })?;
        if matches!(
            lookup,
            "contains" | "startswith" | "endswith" | "regex" | "like"
        ) && descriptor.value_type != ValueType::String
        {
            return Err(py_value_error(format!(
                "unsupported generated manager lookup {lookup:?} for non-string field {field_name:?}"
            )));
        }
        if lookup == "isnull" {
            let is_null = value
                .downcast_exact::<PyBool>()
                .map_err(|_| py_type_error("generated manager isnull lookup requires a bool"))?
                .extract::<bool>()?;
            lowered.push(DynamicExpr::IsNull {
                attr_name: descriptor.attr_name.clone(),
                is_null,
            });
            continue;
        }
        if lookup == "in" {
            let exprs = projected_filter_items(&value, "in")?
                .iter()
                .map(|item| {
                    Ok(DynamicExpr::Compare {
                        attr_name: descriptor.attr_name.clone(),
                        operator: DynamicComparisonOp::Eq,
                        value: projected_filter_attribute_value(
                            py, package, descriptor, field_name, item,
                        )?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            lowered.push(DynamicExpr::Or { exprs });
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
                return Err(py_value_error(format!(
                    "unsupported generated manager lookup {lookup:?}; expected exact, eq, ne, gt, gte, lt, lte, contains, startswith, endswith, regex, in, or isnull"
                )));
            }
        };
        lowered.push(DynamicExpr::Compare {
            attr_name: descriptor.attr_name.clone(),
            operator,
            value: projected_filter_attribute_value(py, package, descriptor, field_name, &value)?,
        });
    }
    Ok(lowered)
}

fn projected_filter_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    descriptor: &OwnedAttributeDescriptor,
    field_name: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<AttributeValue> {
    let expected = package.type_by_label(&descriptor.attr_name, TypeKind::Attribute)?;
    let expected_class = package.class(expected, ProjectedModelForm::Complete)?;
    if value.get_type().as_ptr() == expected_class.bind(py).as_ptr() {
        let scalar = value.call_method0("runtime_attribute_value")?;
        attribute_value_from_py(py, &scalar, descriptor.value_type)
    } else if package.identify_value(py, value).is_ok() {
        Err(py_type_error(format!(
            "generated manager filter {field_name:?} requires its exact attribute wrapper"
        )))
    } else {
        attribute_value_from_py(py, value, descriptor.value_type)
    }
}

fn projected_filter_items<'py>(
    value: &Bound<'py, PyAny>,
    lookup: &str,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    if value.downcast::<PyString>().is_ok() || value.downcast::<PyDict>().is_ok() {
        return Err(py_type_error(format!(
            "generated manager {lookup} lookup requires a non-string iterable"
        )));
    }
    let items = value
        .try_iter()
        .map_err(|_| {
            py_type_error(format!(
                "generated manager {lookup} lookup requires an iterable"
            ))
        })?
        .collect::<PyResult<Vec<_>>>()?;
    if items.is_empty() {
        return Err(py_value_error(format!(
            "generated manager {lookup} lookup requires at least one value"
        )));
    }
    Ok(items)
}

fn projected_filter_iid(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let iid = value
        .downcast::<PyString>()
        .map_err(|_| py_type_error("generated manager IID lookup requires strings"))?
        .to_str()?
        .to_owned();
    if !is_canonical_thing_iid(&iid) {
        return Err(py_value_error(
            "generated manager IID lookup requires a canonical TypeDB thing IID",
        ));
    }
    Ok(iid)
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
    let Some(key) = descriptor.key_attribute() else {
        return Ok(None);
    };
    let values = value.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let Some(wrapper) = values.get_item(&key.field_name)? else {
        return Ok(None);
    };
    if wrapper.is_none() {
        return Ok(None);
    }
    let (wrapper_id, form) = package.identify_value(py, &wrapper)?;
    let expected = package.type_by_label(&key.attr_name, TypeKind::Attribute)?;
    if &wrapper_id != expected || form != ProjectedModelForm::Complete {
        return Err(py_type_error(
            "projected key uses the wrong attribute wrapper",
        ));
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

fn required_projected_iid(value: &Bound<'_, PyAny>) -> PyResult<String> {
    projected_iid(value)?.ok_or_else(|| {
        py_value_error("generated manager update and delete require an attached TypeDB IID")
    })
}

fn resolved_entity_iid(row: Option<&DynamicEntityRow>) -> PyResult<Option<String>> {
    row.map(|row| {
        row.iid
            .clone()
            .ok_or_else(|| py_runtime_error("generated entity identity lookup omitted its IID"))
    })
    .transpose()
}

fn resolved_relation_iid(row: Option<&DynamicRelationRow>) -> PyResult<Option<String>> {
    row.map(|row| {
        row.iid
            .clone()
            .ok_or_else(|| py_runtime_error("generated relation identity lookup omitted its IID"))
    })
    .transpose()
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
                return Err(py_type_error(
                    "projected multi-value input requires a sequence",
                ));
            }
            let tuple = value
                .downcast::<PyTuple>()
                .map_err(|_| py_type_error("projected multi-value input requires a tuple"))?;
            items.extend(tuple.iter());
        }
    }
    let count = u32::try_from(items.len())
        .map_err(|_| py_value_error("projected value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_value_error(
            "projected value violates resolved cardinality",
        ));
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

fn hydrate_validated_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
) -> PyResult<PyObject> {
    let thing = handle.hydrated()?;
    let label = handle.descriptor_type_name(thing.concrete_descriptor())?;
    let id = package
        .types_by_label
        .get(&label)
        .ok_or_else(|| py_runtime_error("query result type is outside the installed projection"))?;
    match thing.kind() {
        ThingKind::Entity if id.kind() == TypeKind::Entity => {
            let descriptor = package
                .projection
                .entity_descriptor(id)
                .map_err(py_orm_error)?;
            let values = hydrate_validated_attributes(
                py,
                package,
                &descriptor.owned_attributes,
                thing.attributes(),
            )?;
            hydrate_complete(py, package, id, &values, Some(thing.concept_id().as_str()))
        }
        ThingKind::Relation if id.kind() == TypeKind::Relation => {
            hydrate_validated_relation(py, package, handle, id, thing)
        }
        _ => Err(py_runtime_error(
            "query result kind conflicts with its installed projection type",
        )),
    }
}

fn hydrate_validated_relation(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
    id: &TypeId,
    thing: &HydratedThing,
) -> PyResult<PyObject> {
    let descriptor = package
        .projection
        .relation_descriptor(id)
        .map_err(py_orm_error)?;
    let values = hydrate_validated_attributes(
        py,
        package,
        &descriptor.owned_attributes,
        thing.attributes(),
    )?;
    let model = &package.projection.projection().models()[id];
    for read in model.complete_read().roles().values() {
        let token = &model.query_tokens().roles()[read.role()];
        let role_name = read.role().label().as_str();
        let role_descriptor = descriptor
            .role(role_name)
            .ok_or_else(|| py_runtime_error("query result role has no provider descriptor"))?;
        let mut players = Vec::new();
        if let Some(role) = thing
            .roles()
            .iter()
            .find(|role| role.role().name == role_name)
        {
            for player in role.players() {
                players.push(hydrate_validated_player(
                    py,
                    package,
                    handle,
                    read.players(),
                    player,
                )?);
            }
        }
        set_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            players,
            role_cardinality(role_descriptor),
        )?;
    }
    hydrate_complete(py, package, id, &values, Some(thing.concept_id().as_str()))
}

fn hydrate_validated_player(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
    allowed: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    player: &HydratedRolePlayer,
) -> PyResult<PyObject> {
    let label = handle.descriptor_type_name(player.concrete_descriptor())?;
    let id = package
        .types_by_label
        .get(&label)
        .ok_or_else(|| py_runtime_error("query role-player type is outside the projection"))?;
    let projected = allowed
        .iter()
        .find(|projected| projected.id() == id)
        .ok_or_else(|| {
            py_runtime_error("query role player is not accepted by the projected role")
        })?;
    let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
        TypeDescriptor::Entity(descriptor) => match projected.form() {
            ProjectedModelForm::Complete => descriptor.owned_attributes.clone(),
            ProjectedModelForm::Reference => descriptor
                .owned_attributes
                .iter()
                .filter(|attribute| attribute.is_key())
                .cloned()
                .collect(),
        },
        TypeDescriptor::Relation(_) => {
            if projected.form() == ProjectedModelForm::Complete {
                return Err(py_runtime_error(
                    "nested complete relation query hydration is forbidden",
                ));
            }
            Vec::new()
        }
    };
    let values = hydrate_validated_attributes(py, package, &descriptors, player.attributes())?;
    match projected.form() {
        ProjectedModelForm::Complete => {
            hydrate_complete(py, package, id, &values, Some(player.concept_id().as_str()))
        }
        ProjectedModelForm::Reference => {
            hydrate_reference(py, package, id, &values, player.concept_id().as_str())
        }
    }
}

fn hydrate_validated_attributes<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &[HydratedAttribute],
) -> PyResult<Bound<'py, PyDict>> {
    let values = PyDict::new(py);
    for descriptor in descriptors {
        let mut wrappers = Vec::new();
        if let Some(attribute) = attributes
            .iter()
            .find(|attribute| attribute.field().name == descriptor.field_name)
        {
            for value in attribute.values() {
                wrappers.push(hydrate_attribute(py, package, descriptor, value)?);
            }
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

fn replace_projected_instance(
    py: Python<'_>,
    instance: Bound<'_, PyAny>,
    hydrated: PyObject,
) -> PyResult<PyObject> {
    let stored = hydrated.bind(py);
    let iid = required_projected_iid(stored)?;
    let values = stored.call_method0("runtime_values")?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    instance.call_method1("attach_runtime_iid", (iid,))?;
    Ok(instance.unbind())
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
        for player in row
            .role_players
            .iter()
            .filter(|player| player.role_name == role_name)
        {
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
        .ok_or_else(|| {
            py_runtime_error("role-player row type is not accepted by the projected role")
        })?;
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
        for (_, value) in attributes
            .iter()
            .filter(|(name, _)| name == &descriptor.attr_name)
        {
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
    let count = u32::try_from(items.len())
        .map_err(|_| py_value_error("hydrated value count exceeds u32"))?;
    if count < minimum || maximum.is_some_and(|maximum| count > maximum) {
        return Err(py_runtime_error(
            "provider row violates projected cardinality",
        ));
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
        return Err(py_runtime_error(
            "exact provider row returned a different concrete type",
        ));
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
        ValueType::Date => {
            exact_temporal_string(py, value, "date", false).map(AttributeValue::Date)
        }
        ValueType::DateTime => {
            exact_temporal_string(py, value, "datetime", false).map(AttributeValue::DateTime)
        }
        ValueType::DateTimeTz => {
            exact_temporal_string(py, value, "datetime", true).map(AttributeValue::DateTimeTZ)
        }
        ValueType::Decimal => {
            exact_module_value_string(py, value, "decimal", "Decimal").map(AttributeValue::Decimal)
        }
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
        return Err(py_type_error(format!(
            "attribute value requires an exact {class_name}"
        )));
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
        return Err(py_type_error(format!(
            "attribute value requires an exact {class_name}"
        )));
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
        return Err(py_value_error(
            "negative projected durations are not representable losslessly",
        ));
    }
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    let fraction = if micros == 0 {
        String::new()
    } else {
        format!(".{micros:06}").trim_end_matches('0').to_owned()
    };
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
    if matches {
        Ok(())
    } else {
        Err(py_runtime_error(
            "provider attribute value type disagrees with the projection",
        ))
    }
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
        AttributeValue::Date(value) => py
            .import("datetime")?
            .getattr("date")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::DateTime(value) | AttributeValue::DateTimeTZ(value) => py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::Decimal(value) => {
            let value = value.strip_suffix("dec").unwrap_or(value);
            py.import("decimal")?
                .getattr("Decimal")?
                .call1((value,))
                .map(Bound::unbind)
        }
        AttributeValue::Duration(value) => duration_to_py(py, value),
    }
}

fn duration_to_py(py: Python<'_>, value: &str) -> PyResult<PyObject> {
    let (days, seconds, micros) = parse_python_day_time_duration(value).ok_or_else(|| {
        py_value_error(
            "duration hydration requires a nonnegative day-time value at microsecond precision",
        )
    })?;
    py.import("datetime")?
        .getattr("timedelta")?
        .call1((days, seconds, micros))
        .map(Bound::unbind)
}

fn parse_python_day_time_duration(value: &str) -> Option<(i64, i64, i64)> {
    let body = value.strip_prefix('P')?;
    if body.is_empty() || body.contains(['Y', 'W']) {
        return None;
    }
    let mut parts = body.split('T');
    let date = parts.next()?;
    let time = parts.next();
    if parts.next().is_some() {
        return None;
    }

    let days = if date.is_empty() {
        0_u64
    } else {
        date.strip_suffix('D')?.parse::<u64>().ok()?
    };
    let mut hours = 0_u64;
    let mut minutes = 0_u64;
    let mut seconds = 0_u64;
    let mut micros = 0_u64;
    let mut saw_component = !date.is_empty();
    if let Some(time) = time {
        if time.is_empty() {
            return None;
        }
        let mut number = String::new();
        let mut last_order = 0_u8;
        for character in time.chars() {
            if character.is_ascii_digit() || character == '.' {
                number.push(character);
                continue;
            }
            if number.is_empty() {
                return None;
            }
            let order = match character {
                'H' => 1,
                'M' => 2,
                'S' => 3,
                _ => return None,
            };
            if order <= last_order {
                return None;
            }
            last_order = order;
            saw_component = true;
            match character {
                'H' => hours = number.parse().ok()?,
                'M' => minutes = number.parse().ok()?,
                'S' => {
                    let (whole, fraction) = number
                        .split_once('.')
                        .map_or((number.as_str(), ""), |parts| parts);
                    if whole.is_empty()
                        || fraction.len() > 9
                        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return None;
                    }
                    seconds = whole.parse().ok()?;
                    let mut nanos = fraction.parse::<u64>().unwrap_or(0);
                    for _ in fraction.len()..9 {
                        nanos *= 10;
                    }
                    if nanos % 1_000 != 0 {
                        return None;
                    }
                    micros = nanos / 1_000;
                }
                _ => unreachable!(),
            }
            number.clear();
        }
        if !number.is_empty() {
            return None;
        }
    }
    if !saw_component {
        return None;
    }

    let seconds = hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    Some((
        i64::try_from(days).ok()?,
        i64::try_from(seconds).ok()?,
        i64::try_from(micros).ok()?,
    ))
}

fn py_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> PyErr {
    py_value_error(error.to_string())
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
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("python-native.yaml").unwrap(), SCHEMA)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap()
    }

    #[test]
    fn provider_decimal_and_day_time_duration_values_convert_losslessly() {
        assert_eq!(
            parse_python_day_time_duration("P1DT2H3M4.000005S"),
            Some((1, 7_384, 5))
        );
        assert_eq!(parse_python_day_time_duration("PT1H"), Some((0, 3_600, 0)));
        assert!(parse_python_day_time_duration("P1M").is_none());
        assert!(parse_python_day_time_duration("PT0.000000001S").is_none());

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let decimal =
                attribute_value_to_py(py, &AttributeValue::Decimal("3.50dec".into())).unwrap();
            assert_eq!(decimal.bind(py).str().unwrap().to_str().unwrap(), "3.50");
            let duration =
                attribute_value_to_py(py, &AttributeValue::Duration("PT3S".into())).unwrap();
            assert_eq!(
                duration
                    .bind(py)
                    .call_method0("total_seconds")
                    .unwrap()
                    .extract::<f64>()
                    .unwrap(),
                3.0
            );
        });
    }

    fn classes(
        py: Python<'_>,
        projection: &RuntimeProjection,
    ) -> Vec<(Py<PyType>, Option<Py<PyType>>)> {
        let module = PyModule::from_code(
            py,
            ffi::c_str!(
                r#"
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
"#
            ),
            ffi::c_str!("projection_models.py"),
            ffi::c_str!("projection_models"),
        )
        .unwrap();
        let builtins = py.import("builtins").unwrap();
        let type_fn = builtins.getattr("type").unwrap();
        projection
            .models()
            .iter()
            .map(|(id, model)| {
                let base = if id.kind() == TypeKind::Attribute {
                    "Attribute"
                } else {
                    "Complete"
                };
                let attrs = PyDict::new(py);
                attrs
                    .set_item("__type_id__", canonical_id(id).unwrap())
                    .unwrap();
                attrs.set_item("__model_form__", "complete").unwrap();
                let bases = PyTuple::new(py, [module.getattr(base).unwrap()]).unwrap();
                let complete = type_fn
                    .call1((model.target_name().as_str(), bases, attrs))
                    .unwrap()
                    .downcast_into::<PyType>()
                    .unwrap()
                    .unbind();
                let reference = model.reference_read().target_name().map(|name| {
                    let attrs = PyDict::new(py);
                    attrs
                        .set_item("__type_id__", canonical_id(id).unwrap())
                        .unwrap();
                    attrs.set_item("__model_form__", "reference").unwrap();
                    let bases = PyTuple::new(py, [module.getattr("Reference").unwrap()]).unwrap();
                    type_fn
                        .call1((name.as_str(), bases, attrs))
                        .unwrap()
                        .downcast_into::<PyType>()
                        .unwrap()
                        .unbind()
                });
                (complete, reference)
            })
            .collect()
    }

    fn install(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let projection = projection();
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();
        let package = install_projection(
            py,
            &projection_json,
            &semantic,
            &fingerprint,
            classes(py, &projection),
        )
        .unwrap();
        (projection, package)
    }

    #[test]
    fn install_is_canonical_tamper_evident_and_requires_exact_coverage() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let projection = projection();
            let projection_json =
                String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic =
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap();
            let fingerprint =
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap();
            install_projection(
                py,
                &projection_json,
                &semantic,
                &fingerprint,
                classes(py, &projection),
            )
            .unwrap();

            let mut missing = classes(py, &projection);
            missing.pop();
            assert!(
                install_projection(py, &projection_json, &semantic, &fingerprint, missing).is_err()
            );

            let mut tampered: serde_json::Value = serde_json::from_str(&projection_json).unwrap();
            tampered["models"][0]["target_name"] = serde_json::json!("Tampered");
            let tampered = String::from_utf8(to_canonical_json(&tampered).unwrap()).unwrap();
            assert!(
                install_projection(
                    py,
                    &tampered,
                    &semantic,
                    &fingerprint,
                    classes(py, &projection)
                )
                .is_err()
            );
        });
    }

    #[test]
    fn native_lowering_and_hydration_preserve_wrappers_iids_and_relation_references() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let identifier_id = package
                .type_by_label("identifier", TypeKind::Attribute)
                .unwrap()
                .clone();
            let aliases_id = package
                .type_by_label("aliases", TypeKind::Attribute)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let identifier_class = package
                .class(&identifier_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let aliases_class = package
                .class(&aliases_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let identifier = identifier_class.call1(("person-1",)).unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("identifier", &identifier).unwrap();
            let person = person_class.call((), Some(&kwargs)).unwrap();
            let descriptor = package.projection.entity_descriptor(&person_id).unwrap();
            assert_eq!(
                lower_attributes(py, package.as_ref(), &descriptor.owned_attributes, &person)
                    .unwrap(),
                vec![(
                    "identifier".into(),
                    AttributeValue::String("person-1".into())
                )]
            );

            let hydrated = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0x-person".into()),
                    type_name: Some("person".into()),
                    attributes: vec![(
                        "identifier".into(),
                        AttributeValue::String("person-1".into()),
                    )],
                },
            )
            .unwrap();
            let hydrated = hydrated.bind(py);
            assert_eq!(
                hydrated
                    .getattr("iid")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0x-person"
            );
            let wrapped = hydrated
                .call_method0("runtime_values")
                .unwrap()
                .downcast::<PyDict>()
                .unwrap()
                .get_item("identifier")
                .unwrap()
                .unwrap();
            assert_eq!(wrapped.get_type().as_ptr(), identifier_class.as_ptr());
            assert_eq!(
                wrapped
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );

            let membership_id = package
                .type_by_label("membership", TypeKind::Relation)
                .unwrap()
                .clone();
            let membership = hydrate_relation(
                py,
                package.as_ref(),
                &membership_id,
                &DynamicRelationRow {
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
                },
            )
            .unwrap();
            let membership_values = membership.bind(py).call_method0("runtime_values").unwrap();
            let member = membership_values
                .downcast::<PyDict>()
                .unwrap()
                .get_item("member")
                .unwrap()
                .unwrap();
            assert_eq!(member.get_type().as_ptr(), person_class.as_ptr());
            assert_eq!(
                member.getattr("iid").unwrap().extract::<String>().unwrap(),
                "0x-person"
            );
            let member_values = member.call_method0("runtime_values").unwrap();
            let member_values = member_values.downcast::<PyDict>().unwrap();
            let member_identifier = member_values.get_item("identifier").unwrap().unwrap();
            assert_eq!(
                member_identifier.get_type().as_ptr(),
                identifier_class.as_ptr()
            );
            assert_eq!(
                member_identifier
                    .call_method0("runtime_attribute_value")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "person-1"
            );
            let member_aliases = member_values.get_item("aliases").unwrap().unwrap();
            let member_aliases = member_aliases.downcast::<PyTuple>().unwrap();
            assert_eq!(member_aliases.len(), 2);
            for alias in member_aliases.iter() {
                assert_eq!(alias.get_type().as_ptr(), aliases_class.as_ptr());
            }

            let container_id = package
                .type_by_label("container", TypeKind::Relation)
                .unwrap()
                .clone();
            let relation = hydrate_relation(
                py,
                package.as_ref(),
                &container_id,
                &DynamicRelationRow {
                    iid: Some("0x-container".into()),
                    type_name: Some("container".into()),
                    attributes: vec![],
                    role_players: vec![DynamicRolePlayer {
                        role_name: "item".into(),
                        player_iid: Some("0x-event".into()),
                        player_type_name: Some("event".into()),
                        attributes: vec![],
                    }],
                },
            )
            .unwrap();
            let values = relation.bind(py).call_method0("runtime_values").unwrap();
            let item = values
                .downcast::<PyDict>()
                .unwrap()
                .get_item("item")
                .unwrap()
                .unwrap();
            let item = item.downcast::<PyTuple>().unwrap().get_item(0).unwrap();
            assert_eq!(
                item.getattr("__model_form__")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference"
            );
            assert_eq!(
                item.getattr("iid").unwrap().extract::<String>().unwrap(),
                "0x-event"
            );
        });
    }
}
