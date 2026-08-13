//! Verified package-scoped runtime projections for generated Python models.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use pyo3::prelude::*;
use pyo3::types::{
    PyAny, PyBool, PyBytes, PyCFunction, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType,
    PyWeakrefMethods, PyWeakrefReference,
};
use pythonize::pythonize;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedContainer, ProjectedModelForm, ProjectedModelUse,
    ProjectedMultiplicity, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_contract::temporal::CanonicalDuration;
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
use type_bridge_orm::{
    HydratedAttribute, HydratedRolePlayer, HydratedThing, ProjectedAttributeValue, ProjectedCreate,
    ProjectedCrudExecutor, ProjectedReference, ProjectedRolePlayer, ProjectedThing, ThingKind,
};
use type_bridge_orm::{InstalledRuntimeProjection, ProviderRuntimeOwner};
use type_bridge_schema::{decode_schema_authority, schema_authority_capability_vocabulary};
use type_bridge_schema_codegen::{PythonEmitter, verify_projection_evidence};

use crate::match_runtime::{
    PyMatchSessionHandle, PyQueryCancellation, PyQueryExecutionResourceLimits, py_sdk_diagnostic,
};
use crate::orm_runtime::{PyRustDatabase, PyRustTransactionContext, provider_block_on};
use crate::validated_result_runtime::PyValidatedMatchThingHandle;

struct RegisteredModel {
    complete: Py<PyType>,
    reference: Option<Py<PyType>>,
}

struct InstalledPackage {
    projection: Arc<InstalledRuntimeProjection>,
    models: BTreeMap<TypeId, RegisteredModel>,
    types_by_label: BTreeMap<String, TypeId>,
    facade_origins: FacadeOriginRegistry,
    named_zone_marker: Option<Py<PyType>>,
}

struct FacadeOriginEntry {
    facade: Py<PyWeakrefReference>,
    proof: FacadeProjectionProof,
}

struct PreparedFacadeOrigin {
    pointer: usize,
    facade: Py<PyWeakrefReference>,
}

struct ProjectedFacadeSnapshot {
    iid: PyObject,
    values: PyObject,
}

#[derive(Clone)]
enum FacadeProjectionProof {
    Thing(Arc<ProjectedThing>),
    Reference(Arc<ProjectedReference>),
}

impl FacadeProjectionProof {
    fn reference(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
        match self {
            Self::Thing(thing) => thing.try_to_reference(installed),
            Self::Reference(reference) => Ok(reference.as_ref().clone()),
        }
    }
}

#[derive(Clone, Default)]
struct FacadeOriginRegistry {
    entries: Arc<Mutex<BTreeMap<usize, FacadeOriginEntry>>>,
}

impl FacadeOriginRegistry {
    fn retain_thing(&self, value: &Bound<'_, PyAny>, thing: Arc<ProjectedThing>) -> PyResult<()> {
        let prepared = self.prepare(value)?;
        self.install(prepared, FacadeProjectionProof::Thing(thing));
        Ok(())
    }

    fn retain_reference(
        &self,
        value: &Bound<'_, PyAny>,
        reference: Arc<ProjectedReference>,
    ) -> PyResult<()> {
        let prepared = self.prepare(value)?;
        self.install(prepared, FacadeProjectionProof::Reference(reference));
        Ok(())
    }

    fn prepare(&self, value: &Bound<'_, PyAny>) -> PyResult<PreparedFacadeOrigin> {
        let pointer = value.as_ptr() as usize;
        let py = value.py();
        let entries = Arc::downgrade(&self.entries);
        let callback =
            PyCFunction::new_closure(py, None, None, move |args, _kwargs| -> PyResult<()> {
                let Some(entries) = entries.upgrade() else {
                    return Ok(());
                };
                let expired = args.get_item(0)?;
                let mut entries = entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if entries.get(&pointer).is_some_and(|entry| {
                    let facade = entry.facade.bind(expired.py());
                    facade.is(&expired) && facade.upgrade().is_none()
                }) {
                    entries.remove(&pointer);
                }
                Ok(())
            })?;
        let facade = PyWeakrefReference::new_with(value, &callback)?.unbind();
        Ok(PreparedFacadeOrigin { pointer, facade })
    }

    fn install(&self, prepared: PreparedFacadeOrigin, proof: FacadeProjectionProof) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(
            prepared.pointer,
            FacadeOriginEntry {
                facade: prepared.facade,
                proof,
            },
        );
    }

    fn proof(&self, value: &Bound<'_, PyAny>) -> Option<FacadeProjectionProof> {
        let pointer = value.as_ptr() as usize;
        let py = value.py();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let retained = entries.get(&pointer).and_then(|entry| {
            entry
                .facade
                .bind(py)
                .upgrade()
                .filter(|facade| facade.is(value))
                .map(|_| entry.proof.clone())
        });
        if retained.is_none() {
            entries.remove(&pointer);
        }
        retained
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
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
    #[pyo3(signature = (
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
        schema_authority = None,
    ))]
    fn new(
        py: Python<'_>,
        projection_json: &str,
        semantic_fingerprint_json: &str,
        projection_fingerprint_json: &str,
        models: Vec<(Py<PyType>, Option<Py<PyType>>)>,
        schema_authority: Option<&Bound<'_, PyBytes>>,
    ) -> PyResult<Self> {
        install_projection(
            py,
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            models,
            schema_authority.map(|authority| authority.as_bytes()),
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
        let value = canonical_attribute_value_from_py(
            py,
            &value,
            projected_value_type(value_type),
            self.package.named_zone_marker.as_ref(),
        )?;
        ProjectedAttributeValue::try_from_attribute_value(&self.package.projection, id, &value)
            .map(|_| ())
            .map_err(py_sdk_diagnostic)
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
        let (actual_id, form) = self.package.identify_value(py, &value).map_err(|_| {
            py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(field.id().clone()),
            ]))
        })?;
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
        let scalar = canonical_attribute_value_from_py(
            py,
            &scalar,
            projected_value_type(value_type),
            self.package.named_zone_marker.as_ref(),
        )?;
        let projected = ProjectedAttributeValue::try_from_attribute_value(
            &self.package.projection,
            attribute_id,
            &scalar,
        )
        .map_err(py_sdk_diagnostic)?;
        self.package
            .projection
            .validate_canonical_field_value(&id, field.id(), projected.value())
            .map_err(py_sdk_diagnostic)
    }

    /// Validate one complete generated create payload through the common Rust contract.
    fn validate_create(
        &self,
        py: Python<'_>,
        model: Py<PyType>,
        instance: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let id = self.package.model_id_for_class(py, &model)?;
        let (instance_id, form) = self.package.identify_value(py, &instance).map_err(|_| {
            py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ]))
        })?;
        if instance_id != id || form != ProjectedModelForm::Complete {
            return Err(py_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_create(py, self.package.as_ref(), &id, &instance).map(|_| ())
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
        Ok(PyMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            type_bridge_orm::QueryExecutionResourceLimits::default(),
            type_bridge_orm::AnswerCancellation::default(),
        ))
    }

    /// Build a match session with one common direct/remote resource policy
    /// and caller-owned cooperative cancellation owner.
    fn match_session_with_resources(
        &self,
        resources: PyRef<'_, PyQueryExecutionResourceLimits>,
        cancellation: PyRef<'_, PyQueryCancellation>,
    ) -> PyResult<PyMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(py_orm_error)?;
        Ok(PyMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            resources.inner(),
            cancellation.inner(),
        ))
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
        self.validate_ordered_create(py, &instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.insert_projected(py, &input)?;
            return self.publish_projected_write(instance, prepared, snapshot, projected);
        }
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
        self.validate_ordered_create(py, &instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.put_projected(py, &input)?;
            return self.publish_projected_write(instance, prepared, snapshot, projected);
        }
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
        self.validate_ordered_create(py, &instance)?;
        let iid = required_projected_iid(&instance)?;
        if self.uses_successor_runtime() {
            let input = project_create(py, self.package.as_ref(), &self.type_id, &instance)?;
            let prepared = self.package.facade_origins.prepare(&instance)?;
            let snapshot = snapshot_projected_instance(&instance)?;
            let projected = self.update_projected(py, &iid, &input)?;
            let hydrated = hydrate_projected_thing_value(py, self.package.as_ref(), &projected)?;
            replace_projected_instance_atomic(py, &instance, hydrated, &snapshot)?;
            self.package
                .facade_origins
                .install(prepared, FacadeProjectionProof::Thing(Arc::new(projected)));
            return Ok(instance.unbind());
        }
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
        if self.uses_successor_runtime() {
            return match self.get_projected(py, iid)? {
                Some(projected) => {
                    hydrate_projected_thing(py, self.package.as_ref(), Arc::new(projected))
                }
                None => Ok(py.None()),
            };
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
    fn uses_successor_runtime(&self) -> bool {
        projection_uses_ordered_collections(self.package.projection.projection())
    }

    fn insert_projected(
        &self,
        py: Python<'_>,
        input: &ProjectedCreate,
    ) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_entity(database.as_ref(), input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_entity_in_transaction(transaction, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_relation(database.as_ref(), input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.insert_relation_in_transaction(transaction, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn put_projected(&self, py: Python<'_>, input: &ProjectedCreate) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_entity(database.as_ref(), input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_entity_in_transaction(transaction, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_relation(database.as_ref(), input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.put_relation_in_transaction(transaction, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn update_projected(
        &self,
        py: Python<'_>,
        iid: &str,
        input: &ProjectedCreate,
    ) -> PyResult<ProjectedThing> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_entity(database.as_ref(), iid, input),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_entity_in_transaction(transaction, iid, input),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_relation(database.as_ref(), iid, input),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.update_relation_in_transaction(transaction, iid, input),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn get_projected(&self, py: Python<'_>, iid: &str) -> PyResult<Option<ProjectedThing>> {
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let result = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_entity_by_iid(database.as_ref(), &self.type_id, iid),
            ),
            (TypeKind::Entity, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_entity_by_iid_in_transaction(transaction, &self.type_id, iid),
            ),
            (TypeKind::Relation, Some(database), None) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_relation_by_iid(database.as_ref(), &self.type_id, iid),
            ),
            (TypeKind::Relation, None, Some(transaction)) => provider_block_on(
                py,
                self.runtime.as_ref(),
                executor.get_relation_by_iid_in_transaction(transaction, &self.type_id, iid),
            ),
            _ => {
                return Err(py_runtime_error(
                    "projected manager has no execution target",
                ));
            }
        };
        result.map_err(py_sdk_diagnostic)
    }

    fn publish_projected_write(
        &self,
        instance: Bound<'_, PyAny>,
        prepared: PreparedFacadeOrigin,
        snapshot: ProjectedFacadeSnapshot,
        projected: ProjectedThing,
    ) -> PyResult<PyObject> {
        let py = instance.py();
        let hydrated = hydrate_projected_thing_value(py, self.package.as_ref(), &projected)?;
        replace_projected_instance_atomic(py, &instance, hydrated, &snapshot)?;
        self.package
            .facade_origins
            .install(prepared, FacadeProjectionProof::Thing(Arc::new(projected)));
        Ok(instance.unbind())
    }

    fn validate_ordered_create(&self, py: Python<'_>, instance: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.uses_successor_runtime() {
            project_create(py, self.package.as_ref(), &self.type_id, instance)?;
        }
        Ok(())
    }

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
    schema_authority: Option<&[u8]>,
) -> PyResult<Arc<InstalledPackage>> {
    let runtime = match schema_authority {
        Some(schema_authority) => install_authority_backed_projection(
            projection_json,
            semantic_fingerprint_json,
            projection_fingerprint_json,
            schema_authority,
        )?,
        None => {
            let runtime = decode_runtime_projection_verified(
                projection_json.as_bytes(),
                semantic_fingerprint_json.as_bytes(),
                projection_fingerprint_json.as_bytes(),
            )
            .map_err(py_diagnostic)?;
            if runtime.target() != BindingTarget::Python {
                return Err(py_runtime_error(
                    "runtime projection does not target Python",
                ));
            }
            if projection_uses_ordered_collections(&runtime) {
                return Err(projection_evidence_mismatch());
            }
            verify_legacy_python_projection_evidence(&runtime)?;
            runtime
        }
    };
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
    let successor = projection_uses_ordered_collections(&runtime);
    let installed = Arc::new(InstalledRuntimeProjection::try_new(runtime).map_err(py_orm_error)?);
    Ok(Arc::new(InstalledPackage {
        projection: installed,
        models: registered,
        types_by_label,
        facade_origins: FacadeOriginRegistry::default(),
        named_zone_marker: successor.then(|| named_zone_marker_class(py)).transpose()?,
    }))
}

fn named_zone_marker_class(py: Python<'_>) -> PyResult<Py<PyType>> {
    PyModule::from_code(
        py,
        pyo3::ffi::c_str!(
            r#"
import datetime as _datetime

class _TypeBridgeNamedZone(_datetime.tzinfo):
    __slots__ = ("_offset", "_type_bridge_zone")

    def __init__(self, offset_seconds, zone):
        self._offset = _datetime.timedelta(seconds=offset_seconds)
        self._type_bridge_zone = zone

    def utcoffset(self, _datetime_value):
        return self._offset

    def dst(self, _datetime_value):
        return _datetime.timedelta(0)

    def tzname(self, _datetime_value):
        return self._type_bridge_zone

    def fromutc(self, datetime_value):
        if datetime_value.tzinfo is not self:
            raise ValueError("fromutc requires this exact timezone marker")
        return datetime_value + self._offset
"#
        ),
        pyo3::ffi::c_str!("_type_bridge_named_zone.py"),
        pyo3::ffi::c_str!("_type_bridge_native"),
    )?
    .getattr("_TypeBridgeNamedZone")?
    .downcast_into::<PyType>()
    .map(Bound::unbind)
    .map_err(|error| py_runtime_error(error.to_string()))
}

fn install_authority_backed_projection(
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    schema_authority: &[u8],
) -> PyResult<RuntimeProjection> {
    let runtime = decode_runtime_projection_verified(
        projection_json.as_bytes(),
        semantic_fingerprint_json.as_bytes(),
        projection_fingerprint_json.as_bytes(),
    )
    .map_err(|_| projection_evidence_mismatch())?;
    if runtime.target() != BindingTarget::Python {
        return Err(projection_evidence_mismatch());
    }
    let authority =
        decode_schema_authority(schema_authority, &schema_authority_capability_vocabulary())
            .map_err(|_| projection_evidence_mismatch())?;
    verify_projection_evidence(&authority, &runtime).map_err(|_| projection_evidence_mismatch())?;
    Ok(runtime)
}

fn projection_evidence_mismatch() -> PyErr {
    py_sdk_diagnostic(SdkExecutionDiagnostic::projection_evidence_mismatch())
}

fn verify_legacy_python_projection_evidence(runtime: &RuntimeProjection) -> PyResult<()> {
    if projection_uses_ordered_collections(runtime) {
        return Err(py_value_error(
            "ordered Python runtime projections require compiled schema authority",
        ));
    }
    let emitter = PythonEmitter::new();
    let resources = emitter.code_resources().map_err(py_diagnostic)?;
    if runtime.config() != &ProjectionConfig::python()
        || runtime.generator_handlers() != emitter.generator_handlers()
        || runtime.code_resources() != resources
    {
        return Err(py_value_error(
            "legacy Python runtime projection does not match the exact shipped handler and resource evidence",
        ));
    }
    Ok(())
}

fn projection_uses_ordered_collections(projection: &RuntimeProjection) -> bool {
    projection.models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
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

fn project_create(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    instance: &Bound<'_, PyAny>,
) -> PyResult<ProjectedCreate> {
    let projection = package.projection.projection();
    let model = projection
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projection model is absent"))?;
    let values = instance.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;

    let mut fields = Vec::with_capacity(model.create().fields().len());
    for field in model.create().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected create field has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        let projected = projected_items(value.as_ref(), field.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                project_attribute_value(
                    py,
                    package,
                    &value,
                    &[
                        SdkDiagnosticPathSegment::Type(id.clone()),
                        SdkDiagnosticPathSegment::Field(field.token().clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::with_capacity(model.create().roles().len());
    for (role_id, role) in model.create().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| py_runtime_error("projected create role has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        let projected = projected_items(value.as_ref(), role.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                project_reference(
                    py,
                    package,
                    &value,
                    role.players(),
                    &[
                        SdkDiagnosticPathSegment::Type(id.clone()),
                        SdkDiagnosticPathSegment::Role(role_id.clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        roles.push((role_id.clone(), projected));
    }

    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles)
        .map_err(py_sdk_diagnostic)
}

fn project_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedAttributeValue> {
    let (attribute_id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if attribute_id.kind() != TypeKind::Attribute || form != ProjectedModelForm::Complete {
        return Err(py_type_error(
            "projected field value is not an exact complete attribute wrapper",
        ));
    }
    let attribute = package
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
    let scalar = canonical_attribute_value_from_py(
        py,
        &scalar,
        projected_value_type(value_type),
        package.named_zone_marker.as_ref(),
    )?;
    ProjectedAttributeValue::try_from_attribute_value(&package.projection, attribute_id, &scalar)
        .map_err(py_sdk_diagnostic)
}

fn project_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    allowed_players: &BTreeSet<ProjectedModelUse>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedReference> {
    let (id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_type_error(
            "projected role player is not an exact entity or relation value",
        ));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(py_type_error(
            "projected role player has an incompatible generated model form",
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| py_runtime_error("projection role-player model is absent"))?;
    let values = value.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| py_runtime_error("projected reference key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| py_runtime_error("projected reference key has no read field"))?;
        let key = values.get_item(token.target_name().as_str())?;
        for (index, item) in projected_items(key.as_ref(), read.multiplicity())?
            .into_iter()
            .enumerate()
        {
            let mut key_path = operation_path.to_vec();
            key_path.extend([
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(key_id.clone()),
                SdkDiagnosticPathSegment::Index(projected_index(index)),
            ]);
            keys.push((
                key_id.clone(),
                project_attribute_value(py, package, &item, &key_path)?,
            ));
        }
    }
    let visible = ProjectedReference::try_new(
        &package.projection,
        id.clone(),
        projected_iid(value)?,
        keys.clone(),
    )
    .map_err(py_sdk_diagnostic)?;
    let Some(proof) = package.facade_origins.proof(value) else {
        return Ok(visible);
    };
    let expected = proof
        .reference(&package.projection)
        .map_err(py_sdk_diagnostic)?;
    if visible != expected {
        return Err(py_sdk_diagnostic(facade_projection_evidence_mismatch(
            operation_path,
        )));
    }
    ProjectedReference::try_new_with_origin_carrier(
        &package.projection,
        id,
        visible.iid().map(str::to_owned),
        keys,
        expected.origin_carrier(),
    )
    .map_err(py_sdk_diagnostic)
}

fn project_hydrated_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: Option<&str>,
) -> PyResult<ProjectedThing> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projection hydrated model is absent"))?;
    let mut fields = Vec::with_capacity(model.complete_read().fields().len());
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| py_runtime_error("projected read field has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        let projected = projected_items(value.as_ref(), field.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                project_hydrated_attribute_value(
                    py,
                    package,
                    &value,
                    &[
                        SdkDiagnosticPathSegment::Type(id.clone()),
                        SdkDiagnosticPathSegment::Field(field.token().clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::with_capacity(model.complete_read().roles().len());
    for (role_id, role) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| py_runtime_error("projected read role has no query token"))?;
        let value = values.get_item(token.target_name().as_str())?;
        let players = projected_items(value.as_ref(), role.multiplicity())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let path = [
                    SdkDiagnosticPathSegment::Type(id.clone()),
                    SdkDiagnosticPathSegment::Role(role_id.clone()),
                    SdkDiagnosticPathSegment::Index(projected_index(index)),
                ];
                let reference =
                    project_hydrated_reference(py, package, &value, role.players(), &path)?;
                ProjectedRolePlayer::try_new(&package.projection, reference)
                    .map_err(py_sdk_diagnostic)
            })
            .collect::<PyResult<Vec<_>>>()?;
        roles.push((role_id.clone(), players));
    }

    ProjectedThing::try_new(
        &package.projection,
        id.clone(),
        iid.unwrap_or_default().to_owned(),
        fields,
        roles,
    )
    .map_err(py_sdk_diagnostic)
}

fn project_hydrated_attribute_value(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedAttributeValue> {
    let (attribute_id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if attribute_id.kind() != TypeKind::Attribute || form != ProjectedModelForm::Complete {
        return Err(py_type_error(
            "hydrated field value is not an exact complete attribute wrapper",
        ));
    }
    let attribute = package
        .projection
        .projection()
        .models()
        .get(&attribute_id)
        .ok_or_else(|| py_runtime_error("projection hydrated attribute is absent"))?;
    let value_type = attribute
        .declaration()
        .value_type()
        .ok_or_else(|| py_runtime_error("projection hydrated attribute has no scalar domain"))?;
    let scalar = value.call_method0("runtime_attribute_value")?;
    let scalar = canonical_attribute_value_from_py(
        py,
        &scalar,
        projected_value_type(value_type),
        package.named_zone_marker.as_ref(),
    )?;
    ProjectedAttributeValue::try_from_hydrated_attribute_value(
        &package.projection,
        attribute_id,
        &scalar,
    )
    .map_err(py_sdk_diagnostic)
}

fn project_hydrated_reference(
    py: Python<'_>,
    package: &InstalledPackage,
    value: &Bound<'_, PyAny>,
    allowed_players: &BTreeSet<ProjectedModelUse>,
    operation_path: &[SdkDiagnosticPathSegment],
) -> PyResult<ProjectedReference> {
    let (id, form) = package.identify_value(py, value).map_err(|_| {
        py_sdk_diagnostic(generated_token_package_mismatch_at(
            operation_path.iter().cloned(),
        ))
    })?;
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_type_error(
            "hydrated role player is not an exact entity or relation value",
        ));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(py_type_error(
            "hydrated role player has an incompatible generated model form",
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| py_runtime_error("projection hydrated role-player model is absent"))?;
    let values = value.call_method0("runtime_values")?;
    let values = values.downcast::<PyDict>()?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| py_runtime_error("projected hydrated key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| py_runtime_error("projected hydrated key has no read field"))?;
        let key = values.get_item(token.target_name().as_str())?;
        for (index, item) in projected_items(key.as_ref(), read.multiplicity())?
            .into_iter()
            .enumerate()
        {
            let mut key_path = operation_path.to_vec();
            key_path.extend([
                SdkDiagnosticPathSegment::Type(id.clone()),
                SdkDiagnosticPathSegment::Field(key_id.clone()),
                SdkDiagnosticPathSegment::Index(projected_index(index)),
            ]);
            keys.push((
                key_id.clone(),
                project_hydrated_attribute_value(py, package, &item, &key_path)?,
            ));
        }
    }
    ProjectedReference::try_new_for_hydration(&package.projection, id, projected_iid(value)?, keys)
        .map_err(py_sdk_diagnostic)
}

fn projected_items<'py>(
    value: Option<&Bound<'py, PyAny>>,
    multiplicity: ProjectedMultiplicity,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    match value {
        None => Ok(Vec::new()),
        Some(value) if value.is_none() => Ok(Vec::new()),
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            Ok(vec![value.clone()])
        }
        Some(value) => value
            .downcast::<PyTuple>()
            .map_err(|_| py_type_error("projected sequence input is not normalized as a tuple"))
            .map(|values| values.iter().collect()),
    }
}

fn generated_token_package_mismatch_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    path.into_iter().fold(
        SdkExecutionDiagnostic::generated_token_package_mismatch(),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .expect("a generated create operation path fits the SDK diagnostic contract")
        },
    )
}

fn facade_projection_evidence_mismatch(
    path: &[SdkDiagnosticPathSegment],
) -> SdkExecutionDiagnostic {
    path.iter().cloned().fold(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("hydrated_facade_evidence_mismatch")
                .expect("static hydrated-facade code is canonical"),
            SdkDiagnosticMessage::new(
                "The hydrated facade no longer matches its retained projection evidence",
            )
            .expect("static hydrated-facade message is canonical"),
        ),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .expect("a generated role-player path fits the SDK diagnostic contract")
        },
    )
}

fn projected_index(index: usize) -> u64 {
    u64::try_from(index).expect("a projected collection index fits the SDK diagnostic contract")
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

fn hydrate_projected_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: Arc<ProjectedThing>,
) -> PyResult<PyObject> {
    let instance = hydrate_projected_thing_value(py, package, projected.as_ref())?;
    package
        .facade_origins
        .retain_thing(instance.bind(py), projected)?;
    Ok(instance)
}

fn hydrate_projected_thing_value(
    py: Python<'_>,
    package: &InstalledPackage,
    projected: &ProjectedThing,
) -> PyResult<PyObject> {
    projected
        .validate_for(package.projection.as_ref())
        .map_err(py_sdk_diagnostic)?;
    let id = projected.type_id();
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(py_runtime_error(
            "projected thing hydration requires an entity or relation",
        ));
    }
    let values = hydrate_projected_fields(py, package, id, projected.fields(), false)?;
    if id.kind() == TypeKind::Relation {
        let model = package
            .projection
            .projection()
            .models()
            .get(id)
            .ok_or_else(|| py_runtime_error("projected relation model is absent"))?;
        for (role_id, read) in model.complete_read().roles() {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                py_runtime_error("projected relation read role has no query token")
            })?;
            let players = projected.roles().get(role_id).ok_or_else(|| {
                py_runtime_error("projected relation omitted a validated read role")
            })?;
            let mut hydrated = Vec::new();
            hydrated
                .try_reserve(players.len())
                .map_err(|_| py_runtime_error("projected relation role allocation failed"))?;
            for player in players {
                hydrated.push(hydrate_projected_player(py, package, player)?);
            }
            set_projected_hydrated_values(
                py,
                &values,
                token.target_name().as_str(),
                hydrated,
                read.multiplicity(),
            )?;
        }
    } else if !projected.roles().is_empty() {
        return Err(py_runtime_error(
            "projected entity unexpectedly contains relation roles",
        ));
    }
    allocate_projected_complete(py, package, id, &values, projected.iid())
}

fn hydrate_projected_player(
    py: Python<'_>,
    package: &InstalledPackage,
    player: &ProjectedRolePlayer,
) -> PyResult<PyObject> {
    player
        .validate_for(package.projection.as_ref())
        .map_err(py_sdk_diagnostic)?;
    let reference = Arc::new(player.reference().clone());
    let instance = match player.exact_form() {
        Some(ProjectedModelForm::Complete) => {
            if player.type_id().kind() != TypeKind::Entity {
                return Err(py_sdk_diagnostic(projected_role_player_form_missing(
                    player.type_id(),
                )));
            }
            let values =
                hydrate_projected_fields(py, package, player.type_id(), player.fields(), false)?;
            allocate_projected_complete(py, package, player.type_id(), &values, player.iid())?
        }
        Some(ProjectedModelForm::Reference) => {
            let fields = player
                .keys()
                .iter()
                .map(|(field, value)| (field.clone(), vec![value.clone()]))
                .collect::<BTreeMap<_, _>>();
            let values = hydrate_projected_fields(py, package, player.type_id(), &fields, true)?;
            allocate_projected_reference(py, package, player.type_id(), &values, player.iid())?
        }
        None => {
            return Err(py_sdk_diagnostic(projected_role_player_form_missing(
                player.type_id(),
            )));
        }
    };
    package
        .facade_origins
        .retain_reference(instance.bind(py), reference)?;
    Ok(instance)
}

fn hydrate_projected_fields<'py>(
    py: Python<'py>,
    package: &InstalledPackage,
    id: &TypeId,
    fields: &BTreeMap<type_bridge_contract::schema::OwnsFactId, Vec<ProjectedAttributeValue>>,
    reference: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| py_runtime_error("projected hydrated model is absent"))?;
    let descriptors = match package.projection.descriptor(id).map_err(py_orm_error)? {
        TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
    };
    let selected = model
        .complete_read()
        .fields()
        .iter()
        .filter(|field| !reference || model.reference_read().key_fields().contains(field.token()))
        .collect::<Vec<_>>();
    if fields.len() != selected.len() {
        return Err(py_runtime_error(
            "projected hydrated fields do not match the selected model form",
        ));
    }
    let values = PyDict::new(py);
    for read in selected {
        let field_id = read.token();
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| py_runtime_error("projected hydrated field has no query token"))?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.field_name == token.target_name().as_str()
                    && descriptor.attr_name == field_id.attribute().label().as_str()
            })
            .ok_or_else(|| py_runtime_error("projected hydrated field has no descriptor"))?;
        let projected_values = fields
            .get(field_id)
            .ok_or_else(|| py_runtime_error("projected hydrated model omitted a selected field"))?;
        let mut hydrated = Vec::new();
        hydrated
            .try_reserve(projected_values.len())
            .map_err(|_| py_runtime_error("projected hydrated field allocation failed"))?;
        for value in projected_values {
            if value.attribute_type().label().as_str() != descriptor.attr_name {
                return Err(py_runtime_error(
                    "projected hydrated scalar has the wrong attribute type",
                ));
            }
            hydrated.push(hydrate_attribute(
                py,
                package,
                descriptor,
                &value.to_attribute_value(),
            )?);
        }
        set_projected_hydrated_values(
            py,
            &values,
            token.target_name().as_str(),
            hydrated,
            read.multiplicity(),
        )?;
    }
    Ok(values)
}

fn set_projected_hydrated_values(
    py: Python<'_>,
    values: &Bound<'_, PyDict>,
    name: &str,
    items: Vec<PyObject>,
    multiplicity: ProjectedMultiplicity,
) -> PyResult<()> {
    let cardinality = multiplicity.cardinality();
    let count = u64::try_from(items.len())
        .map_err(|_| py_runtime_error("projected hydrated value count exceeds u64"))?;
    if count < cardinality.min() || cardinality.max().is_some_and(|maximum| count > maximum) {
        return Err(py_runtime_error(
            "projected hydrated value violates its projected cardinality",
        ));
    }
    match multiplicity.container() {
        ProjectedContainer::Scalar => match items.into_iter().next() {
            Some(value) => values.set_item(name, value),
            None => values.set_item(name, py.None()),
        },
        ProjectedContainer::Sequence => values.set_item(name, PyTuple::new(py, items)?),
    }
}

fn allocate_projected_complete(
    py: Python<'_>,
    package: &InstalledPackage,
    id: &TypeId,
    values: &Bound<'_, PyDict>,
    iid: &str,
) -> PyResult<PyObject> {
    let instance = allocate(py, package.class(id, ProjectedModelForm::Complete)?)?;
    instance.call_method1("initialize_runtime_values", (values,))?;
    instance.call_method1("attach_runtime_iid", (iid,))?;
    Ok(instance.unbind())
}

fn allocate_projected_reference(
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

fn projected_role_player_form_missing(type_id: &TypeId) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(
        SdkDiagnosticCode::new("hydrated_role_player_form_missing")
            .expect("static projected-role code is canonical"),
        SdkDiagnosticMessage::new(
            "Successor hydration requires exact complete-or-reference role-player evidence",
        )
        .expect("static projected-role message is canonical"),
    )
    .try_at(SdkDiagnosticPathSegment::Type(type_id.clone()))
    .expect("a projected role-player type path is bounded")
}

fn hydrate_validated_thing(
    py: Python<'_>,
    package: &InstalledPackage,
    handle: &PyValidatedMatchThingHandle,
) -> PyResult<PyObject> {
    if let Some(projected) = handle.projected()? {
        return hydrate_projected_thing(py, package, projected);
    }
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

fn snapshot_projected_instance(instance: &Bound<'_, PyAny>) -> PyResult<ProjectedFacadeSnapshot> {
    Ok(ProjectedFacadeSnapshot {
        iid: instance.getattr("_iid")?.unbind(),
        values: instance.getattr("_values")?.unbind(),
    })
}

fn replace_projected_instance_atomic(
    py: Python<'_>,
    instance: &Bound<'_, PyAny>,
    hydrated: PyObject,
    snapshot: &ProjectedFacadeSnapshot,
) -> PyResult<()> {
    if let Err(error) = replace_projected_instance(py, instance.clone(), hydrated) {
        if let Err(rollback) = restore_projected_instance(instance, snapshot) {
            return Err(py_runtime_error(format!(
                "projected facade replacement failed and rollback failed: {error}; {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn restore_projected_instance(
    instance: &Bound<'_, PyAny>,
    snapshot: &ProjectedFacadeSnapshot,
) -> PyResult<()> {
    let py = instance.py();
    let values_error = instance.setattr("_values", snapshot.values.bind(py)).err();
    let iid_error = instance.setattr("_iid", snapshot.iid.bind(py)).err();
    match (values_error, iid_error) {
        (None, None) => Ok(()),
        (Some(error), None) | (None, Some(error)) => Err(error),
        (Some(values), Some(iid)) => Err(py_runtime_error(format!(
            "projected facade values and IID rollback both failed: {values}; {iid}"
        ))),
    }
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
    if projection_uses_ordered_collections(package.projection.projection()) {
        ProjectedAttributeValue::try_from_hydrated_attribute_value(
            &package.projection,
            id.clone(),
            value,
        )
        .map_err(py_sdk_diagnostic)?;
    }
    let class = package.class(id, ProjectedModelForm::Complete)?;
    let scalar = attribute_value_to_py(py, value, package.named_zone_marker.as_ref())?;
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
    if projection_uses_ordered_collections(package.projection.projection()) {
        project_hydrated_thing(py, package, id, values, iid)?;
    }
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

fn canonical_attribute_value_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    value_type: ValueType,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<AttributeValue> {
    let value = match value_type {
        ValueType::DateTime => exact_temporal_string(py, value, "datetime", false)
            .map(|value| AttributeValue::DateTime(canonical_python_datetime(&value, false))),
        ValueType::DateTimeTz if named_zone_marker.is_some() => {
            canonical_python_datetime_tz(py, value, named_zone_marker)
                .map(AttributeValue::DateTimeTZ)
        }
        ValueType::DateTimeTz => exact_temporal_string(py, value, "datetime", true)
            .map(|value| AttributeValue::DateTimeTZ(canonical_python_datetime(&value, true))),
        ValueType::Duration => canonical_duration_from_py(py, value).map(AttributeValue::Duration),
        _ => attribute_value_from_py(py, value, value_type),
    };
    value.map_err(|_| {
        py_sdk_diagnostic(SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ))
    })
}

fn canonical_python_datetime_tz(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<String> {
    let value_text = exact_temporal_string(py, value, "datetime", true)?;
    let canonical = canonical_python_datetime(&value_text, true);
    let timezone = value.getattr("tzinfo")?;
    if named_zone_marker.is_some_and(|marker| timezone.get_type().is(marker.bind(py))) {
        let zone = timezone.getattr("_type_bridge_zone")?.extract::<String>()?;
        return Ok(format!("{canonical}[{zone}]"));
    }
    let zoneinfo = py.import("zoneinfo")?.getattr("ZoneInfo")?;
    if timezone.get_type().as_ptr() != zoneinfo.as_ptr() {
        return Ok(canonical);
    }
    let key = timezone.getattr("key")?.extract::<String>()?;
    Ok(format!("{canonical}[{key}]"))
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

fn canonical_python_datetime(value: &str, timezone_required: bool) -> String {
    let Some(tail) = value.get(19..) else {
        return value.to_owned();
    };
    let suffix_start = timezone_required
        .then(|| {
            tail.char_indices()
                .find_map(|(index, character)| matches!(character, '+' | '-').then_some(19 + index))
        })
        .flatten()
        .unwrap_or(value.len());
    let (local, suffix) = value.split_at(suffix_start);
    let local = local.rsplit_once('.').map_or_else(
        || local.to_owned(),
        |(whole, fraction)| {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_owned()
            } else {
                format!("{whole}.{fraction}")
            }
        },
    );
    if suffix == "+00:00" {
        format!("{local}Z")
    } else {
        format!("{local}{suffix}")
    }
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
    let (days, seconds, micros) = duration_components_from_py(py, value)?;
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

fn canonical_duration_from_py(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let (days, seconds, micros) = duration_components_from_py(py, value)?;
    CanonicalDuration::new(
        false,
        0,
        u64::try_from(days).expect("a nonnegative Python timedelta day count fits u64"),
        u64::try_from(seconds).expect("a nonnegative Python timedelta second count fits u64"),
        u32::try_from(micros).expect("Python timedelta microseconds fit u32") * 1_000,
    )
    .map(|duration| duration.to_string())
    .map_err(py_diagnostic)
}

fn duration_components_from_py(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<(i64, i64, i64)> {
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
    Ok((days, seconds, micros))
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

fn attribute_value_to_py(
    py: Python<'_>,
    value: &AttributeValue,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<PyObject> {
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
        AttributeValue::DateTime(value) => py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind),
        AttributeValue::DateTimeTZ(value) if named_zone_marker.is_some() => {
            datetime_tz_to_py(py, value, named_zone_marker)
        }
        AttributeValue::DateTimeTZ(value) => py
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

fn datetime_tz_to_py(
    py: Python<'_>,
    value: &str,
    named_zone_marker: Option<&Py<PyType>>,
) -> PyResult<PyObject> {
    let Some((evidence, zone)) = value
        .strip_suffix(']')
        .and_then(|value| value.rsplit_once('['))
    else {
        return py
            .import("datetime")?
            .getattr("datetime")?
            .call_method1("fromisoformat", (value,))
            .map(Bound::unbind);
    };
    let marker = named_zone_marker
        .ok_or_else(|| py_runtime_error("named-zone hydration requires successor evidence"))?;
    let resolved = py
        .import("datetime")?
        .getattr("datetime")?
        .call_method1("fromisoformat", (evidence,))?;
    let offset_seconds = python_timedelta_integral_seconds(&resolved.call_method0("utcoffset")?)?;
    let timezone = marker.bind(py).call1((offset_seconds, zone))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("tzinfo", timezone)?;
    resolved
        .call_method("replace", (), Some(&kwargs))
        .map(Bound::unbind)
}

fn python_timedelta_integral_seconds(value: &Bound<'_, PyAny>) -> PyResult<i32> {
    let days = value.getattr("days")?.extract::<i32>()?;
    let seconds = value.getattr("seconds")?.extract::<i32>()?;
    let microseconds = value.getattr("microseconds")?.extract::<i32>()?;
    if microseconds != 0 {
        return Err(py_runtime_error(
            "timezone offsets must resolve to an integral number of seconds",
        ));
    }
    days.checked_mul(86_400)
        .and_then(|days| days.checked_add(seconds))
        .ok_or_else(|| py_runtime_error("timezone offset exceeds the supported range"))
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
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use pyo3::ffi;
    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{
        BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_core_lib::ast::{TypedFetchRows, TypedHydrateThings};
    use type_bridge_orm::session::backend::{
        AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
        BoundedAnswerStats, BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };
    use type_bridge_orm::{
        AnswerCancellation, ClassifiedCommitError, DatabaseConnectionAuthority, OrmError,
        ProjectedQueryMaterializationLimits, ProjectedQueryOrigin, QueryExecutionDeadline,
        QueryExecutionResourceLimits, RowCardinality, SessionHandle, TxType, Window,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        VerifiedSchemaAuthority, build_schema_authority, encode_schema_authority,
        normalize_documents, project,
    };
    use type_bridge_schema_codegen::{PythonEmitter, TypeScriptEmitter};

    use super::*;
    use crate::match_runtime::{PyQueryInvocationBudget, validated_result_handle};

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

    const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  tag:
    value:
      type: string
      regex: "^.+$"
entities:
  actor:
    abstract: true
    owns:
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  person:
    sub: actor
relations:
  activity:
    relates:
      participant:
        abstract: true
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  gathering:
    sub: activity
  container:
    relates:
      item:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
plays:
  person:
    activity: [participant]
  gathering:
    container: [item]
"#;

    #[derive(Debug, Default)]
    struct OriginRecordingState {
        opens: Vec<TxType>,
        queries: Vec<String>,
        commits: usize,
        rollbacks: usize,
        closes: usize,
    }

    struct OriginRecordingBackend {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        state: Arc<Mutex<OriginRecordingState>>,
    }

    #[derive(Debug, Default)]
    struct QueryOriginState {
        opens: Vec<TxType>,
        selected: usize,
        hydrated: usize,
        closes: usize,
    }

    struct QueryOriginBackend {
        state: Arc<Mutex<QueryOriginState>>,
        iid: &'static str,
        kind: QueryOriginKind,
    }

    #[derive(Clone, Copy)]
    enum QueryOriginKind {
        Person,
        Gathering,
    }

    impl QueryOriginBackend {
        fn new(iid: &'static str) -> (Self, Arc<Mutex<QueryOriginState>>) {
            let state = Arc::new(Mutex::new(QueryOriginState::default()));
            (
                Self {
                    state: Arc::clone(&state),
                    iid,
                    kind: QueryOriginKind::Person,
                },
                state,
            )
        }

        fn gathering(iid: &'static str) -> (Self, Arc<Mutex<QueryOriginState>>) {
            let state = Arc::new(Mutex::new(QueryOriginState::default()));
            (
                Self {
                    state: Arc::clone(&state),
                    iid,
                    kind: QueryOriginKind::Gathering,
                },
                state,
            )
        }
    }

    impl DriverBackend for QueryOriginBackend {
        fn match_capabilities(&self) -> type_bridge_orm::CapabilitySet {
            type_bridge_orm::CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = QueryOriginTransaction {
                state: Arc::clone(&self.state),
                iid: self.iid,
                kind: self.kind,
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct QueryOriginTransaction {
        state: Arc<Mutex<QueryOriginState>>,
        iid: &'static str,
        kind: QueryOriginKind,
    }

    impl QueryOriginTransaction {
        fn selected(
            &self,
            limits: BoundedAnswerLimits,
            consumer: &mut dyn AnswerConsumer,
        ) -> Result<BoundedAnswerStats, OrmError> {
            self.state.lock().unwrap().selected += 1;
            feed_query_origin(
                vec![AnswerItem::Row(serde_json::json!({
                    "bindings": [{"binding": 0, "concept_id": self.iid}],
                    "satisfied_role_edges": [],
                }))],
                limits,
                consumer,
            )
        }
    }

    impl TransactionOps for QueryOriginTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("successor Python query-origin test used a string query") })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move { self.selected(limits, consumer) })
        }

        fn query_tuple_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move { self.selected(limits, consumer) })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.state.lock().unwrap().hydrated += 1;
            let iid = self.iid;
            let document = match self.kind {
                QueryOriginKind::Person => serde_json::json!({
                    "binding": 0,
                    "concept_id": iid,
                    "concrete_type": "person",
                    "kind": "entity",
                    "attributes": [{
                        "field": "tag",
                        "value_type": "string",
                        "values": ["query-tag"],
                    }],
                    "roles": [],
                }),
                QueryOriginKind::Gathering => serde_json::json!({
                    "binding": 0,
                    "concept_id": iid,
                    "concrete_type": "gathering",
                    "kind": "relation",
                    "attributes": [],
                    "roles": [{
                        "role": "participant",
                        "players": [{
                            "concept_id": "0xa6",
                            "declared_type": "person",
                            "concrete_type": "person",
                            "kind": "entity",
                            "attributes": [{
                                "field": "tag",
                                "value_type": "string",
                                "values": ["nested-query-tag"],
                            }],
                        }],
                    }],
                }),
            };
            Box::pin(async move {
                feed_query_origin(vec![AnswerItem::Document(document)], limits, consumer)
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    fn feed_query_origin(
        items: Vec<AnswerItem>,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, OrmError> {
        let mut reader = BoundedAnswerReader::new(limits);
        reader.check_before_read()?;
        for item in items {
            if reader.accept(item, consumer)? == AnswerControl::Stop {
                break;
            }
        }
        Ok(reader.stats())
    }

    impl OriginRecordingBackend {
        fn new(responses: Vec<QueryResult>) -> (Self, Arc<Mutex<OriginRecordingState>>) {
            let state = Arc::new(Mutex::new(OriginRecordingState::default()));
            (
                Self {
                    responses: Arc::new(Mutex::new(responses.into())),
                    state: Arc::clone(&state),
                },
                state,
            )
        }
    }

    impl DriverBackend for OriginRecordingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = OriginRecordingTransaction {
                responses: Arc::clone(&self.responses),
                state: Arc::clone(&self.state),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct OriginRecordingTransaction {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        state: Arc<Mutex<OriginRecordingState>>,
    }

    impl TransactionOps for OriginRecordingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("successor Python origin test used a legacy query") })
        }

        fn query_canonical(
            &mut self,
            typeql: &str,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.state.lock().unwrap().queries.push(typeql.to_owned());
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("successor Python origin test issued unexpected provider I/O");
            Box::pin(async move { Ok(response) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("successor Python origin test used the lossy commit path") })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    fn authority(source: &str, document: &str) -> VerifiedSchemaAuthority {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new(document).unwrap(), source)]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|id| CapabilityId::new(*id).unwrap())
            .collect();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("python-native").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            available,
        );
        build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap()
    }

    fn python_projection(authority: &VerifiedSchemaAuthority) -> RuntimeProjection {
        let emitter = PythonEmitter::new();
        project(
            authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
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
                attribute_value_to_py(py, &AttributeValue::Decimal("3.50dec".into()), None)
                    .unwrap();
            assert_eq!(decimal.bind(py).str().unwrap().to_str().unwrap(), "3.50");
            let duration =
                attribute_value_to_py(py, &AttributeValue::Duration("PT3S".into()), None).unwrap();
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

    #[test]
    fn python_datetime_isoformats_normalize_without_changing_nonzero_offsets() {
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000", false),
            "2026-07-29T01:02:03.12"
        );
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000+00:00", true),
            "2026-07-29T01:02:03.12Z"
        );
        assert_eq!(
            canonical_python_datetime("2026-07-29T01:02:03.120000+05:30", true),
            "2026-07-29T01:02:03.12+05:30"
        );
    }

    #[test]
    fn named_zone_hydration_preserves_both_dst_overlap_instants_without_host_tzdb() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let marker = package.named_zone_marker.as_ref();
            for (evidence, expected) in [
                (
                    "2026-11-01T01:30:00-04:00[America/New_York]",
                    "2026-11-01T01:30:00-04:00",
                ),
                (
                    "2026-11-01T01:30:00-05:00[America/New_York]",
                    "2026-11-01T01:30:00-05:00",
                ),
            ] {
                let hydrated = datetime_tz_to_py(py, evidence, marker).unwrap();
                assert_eq!(
                    hydrated
                        .bind(py)
                        .call_method0("isoformat")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    expected
                );
                let timezone_type = py.import("datetime").unwrap().getattr("timezone").unwrap();
                assert!(
                    !hydrated
                        .bind(py)
                        .getattr("tzinfo")
                        .unwrap()
                        .is_instance(&timezone_type)
                        .unwrap()
                );
                assert_eq!(
                    canonical_python_datetime_tz(py, hydrated.bind(py), marker).unwrap(),
                    evidence
                );
            }
        });
    }

    #[test]
    fn unordered_datetime_tz_keeps_offset_only_predecessor_behavior() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let authored = py
                .import("datetime")
                .unwrap()
                .getattr("datetime")
                .unwrap()
                .call_method1("fromisoformat", ("2026-11-01T01:30:00-05:00",))
                .unwrap();
            assert_eq!(
                canonical_attribute_value_from_py(py, &authored, ValueType::DateTimeTz, None)
                    .unwrap(),
                AttributeValue::DateTimeTZ("2026-11-01T01:30:00-05:00".into())
            );

            let hydrated = attribute_value_to_py(
                py,
                &AttributeValue::DateTimeTZ("2026-11-01T01:30:00-05:00".into()),
                None,
            )
            .unwrap();
            assert!(
                hydrated
                    .bind(py)
                    .getattr("tzinfo")
                    .unwrap()
                    .getattr("key")
                    .is_err(),
                "legacy unordered hydration must not upgrade offset evidence to ZoneInfo",
            );
            assert!(
                attribute_value_to_py(
                    py,
                    &AttributeValue::DateTimeTZ(
                        "2026-11-01T01:30:00-05:00[America/New_York]".into(),
                    ),
                    None,
                )
                .is_err(),
                "legacy unordered hydration keeps rejecting bracketed named-zone evidence",
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
        let authority = authority(SCHEMA, "python-native.yaml");
        let projection = python_projection(&authority);
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
            None,
        )
        .unwrap();
        (projection, package)
    }

    fn install_ordered(py: Python<'_>) -> (RuntimeProjection, Arc<InstalledPackage>) {
        let authority = authority(ORDERED_SCHEMA, "python-ordered-native.yaml");
        let authority_bytes = encode_schema_authority(&authority);
        let projection = python_projection(&authority);
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
            Some(&authority_bytes),
        )
        .unwrap();
        (projection, package)
    }

    fn origin_person_document(iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "person",
            "attributes": {
                "tag": [{"value": "source-tag"}]
            }
        })
    }

    fn origin_gathering_document(iid: &str, player_iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "gathering",
            "attributes": {},
            "role_players": [{
                "role_name": "participant",
                "player_iid": player_iid,
                "player_type_name": "person",
                "attributes": {
                    "tag": [{"value": "source-tag"}]
                }
            }]
        })
    }

    fn origin_container_document(iid: &str, player_iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "container",
            "attributes": {},
            "role_players": [{
                "role_name": "item",
                "player_iid": player_iid,
                "player_type_name": "gathering",
                "attributes": {}
            }]
        })
    }

    fn origin_manager(
        package: Arc<InstalledPackage>,
        type_id: TypeId,
        database: Option<Arc<Database>>,
        transaction: Option<TransactionContext>,
        runtime: Arc<ProviderRuntimeOwner>,
    ) -> PyProjectedModelManager {
        PyProjectedModelManager {
            package,
            type_id,
            database,
            transaction,
            runtime,
            filters: vec![],
        }
    }

    fn thing_query(
        package: &InstalledPackage,
        type_name: &str,
    ) -> (
        Arc<type_bridge_orm::_registry::DescriptorRegistry>,
        type_bridge_orm::ValidatedMatchRequest,
    ) {
        let registry = Arc::new(package.projection.match_registry().unwrap());
        let session = SessionHandle::new(Arc::clone(&registry));
        let thing = session.exact(type_name).unwrap();
        let shape = session.positional([thing.one()]).unwrap();
        let query = session.query(shape).unwrap();
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
        (registry, validated)
    }

    fn person_query(
        package: &InstalledPackage,
    ) -> (
        Arc<type_bridge_orm::_registry::DescriptorRegistry>,
        type_bridge_orm::ValidatedMatchRequest,
    ) {
        thing_query(package, "person")
    }

    fn query_budget() -> PyQueryInvocationBudget {
        let resources = QueryExecutionResourceLimits::default();
        PyQueryInvocationBudget::from_parts(
            resources,
            QueryExecutionDeadline::for_limits(resources),
            AnswerCancellation::default(),
        )
    }

    fn hydrate_first_query_thing(
        py: Python<'_>,
        package: &InstalledPackage,
        handle: crate::validated_result_runtime::PyValidatedMatchResultHandle,
    ) -> PyObject {
        let handle = Py::new(py, handle).unwrap();
        let row = handle.bind(py).call_method1("row", (0,)).unwrap();
        let slot = row.call_method1("slot", (0,)).unwrap();
        let thing = slot.call_method1("thing", (0,)).unwrap();
        let thing = thing
            .extract::<PyRef<'_, PyValidatedMatchThingHandle>>()
            .unwrap();
        hydrate_validated_thing(py, package, &thing).unwrap()
    }

    fn gathering_with_player<'py>(
        py: Python<'py>,
        package: &InstalledPackage,
        gathering_id: &TypeId,
        player: &Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        relation_with_player(py, package, gathering_id, "participant", player)
    }

    fn relation_with_player<'py>(
        py: Python<'py>,
        package: &InstalledPackage,
        relation_id: &TypeId,
        role: &str,
        player: &Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        let kwargs = PyDict::new(py);
        kwargs
            .set_item(role, PyTuple::new(py, [player]).unwrap())
            .unwrap();
        package
            .class(relation_id, ProjectedModelForm::Complete)
            .unwrap()
            .bind(py)
            .call((), Some(&kwargs))
            .unwrap()
    }

    #[test]
    fn install_is_canonical_tamper_evident_and_requires_exact_coverage() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let authority = authority(SCHEMA, "python-native.yaml");
            let projection = python_projection(&authority);
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
                None,
            )
            .unwrap();

            let mut missing = classes(py, &projection);
            missing.pop();
            assert!(
                install_projection(py, &projection_json, &semantic, &fingerprint, missing, None,)
                    .is_err()
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
                    classes(py, &projection),
                    None,
                )
                .is_err()
            );
        });
    }

    #[test]
    fn install_rejects_a_foreign_binding_target_before_registration() {
        let authority = authority(SCHEMA, "typescript-native.yaml");
        let emitter = TypeScriptEmitter::new();
        let projection = project(
            authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let error =
                install_projection(py, &projection_json, &semantic, &fingerprint, vec![], None)
                    .err()
                    .expect("a TypeScript projection must not install as Python");
            assert!(
                error
                    .to_string()
                    .contains("runtime projection does not target Python")
            );
        });
    }

    #[test]
    fn whole_create_rejects_a_shape_compatible_foreign_fieldless_instance() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (projection, local_package) = install(py);
            let projection_json =
                String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
            let semantic =
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap();
            let fingerprint =
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap();
            let foreign_package = install_projection(
                py,
                &projection_json,
                &semantic,
                &fingerprint,
                classes(py, &projection),
                None,
            )
            .unwrap();
            let event_id = local_package
                .type_by_label("event", TypeKind::Relation)
                .unwrap()
                .clone();
            let local_class = local_package
                .class(&event_id, ProjectedModelForm::Complete)
                .unwrap()
                .clone_ref(py);
            let local_instance = local_class.bind(py).call0().unwrap();
            let runtime = PyRuntimeProjection {
                package: local_package,
            };
            runtime
                .validate_create(py, local_class.clone_ref(py), local_instance)
                .unwrap();

            let foreign_instance = foreign_package
                .class(&event_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call0()
                .unwrap();
            let error = runtime
                .validate_create(py, local_class, foreign_instance)
                .unwrap_err();
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "generated_token_package_mismatch"
            );
        });
    }

    #[test]
    fn ordered_single_writes_reject_invalid_whole_creates_before_execution_target_resolution() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let tag_id = package
                .type_by_label("tag", TypeKind::Attribute)
                .unwrap()
                .clone();
            let person_class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let tag_class = package
                .class(&tag_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let duplicate = tag_class.call1(("duplicate",)).unwrap();
            let tags = PyTuple::new(py, [duplicate.clone(), duplicate]).unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("tag", tags).unwrap();
            let invalid = person_class.call((), Some(&kwargs)).unwrap();
            invalid
                .call_method1("attach_runtime_iid", ("0xa",))
                .unwrap();
            let manager = PyProjectedModelManager {
                package,
                type_id: person_id,
                database: None,
                transaction: None,
                runtime: Arc::new(
                    ProviderRuntimeOwner::new().expect("provider runtime should start"),
                ),
                filters: Vec::new(),
            };

            let errors = [
                manager.insert(py, invalid.clone()).unwrap_err(),
                manager.put(py, invalid.clone()).unwrap_err(),
                manager.update(py, invalid).unwrap_err(),
            ];
            for error in errors {
                assert!(!error.to_string().contains("no execution target"));
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "invalid_input"
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    "ordered_distinct_duplicate"
                );
            }
        });
    }

    #[test]
    fn ordered_hydration_rejects_duplicate_scalars_and_players_with_integrity_paths() {
        use pythonize::depythonize;

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let person_model = &package.projection.projection().models()[&person_id];
            let tag = person_model.complete_read().fields()[0].token().clone();
            let participant = package.projection.projection().models()[&gathering_id]
                .complete_read()
                .roles()
                .keys()
                .next()
                .unwrap()
                .clone();

            let scalar_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        ("tag".into(), AttributeValue::String("same".into())),
                        ("tag".into(), AttributeValue::String("same".into())),
                    ],
                },
            )
            .unwrap_err();
            let scalar = scalar_error.value(py);
            assert_eq!(
                scalar
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                scalar.getattr("code").unwrap().extract::<String>().unwrap(),
                "ordered_distinct_duplicate"
            );
            let scalar_path: serde_json::Value =
                depythonize(&scalar.getattr("path").unwrap()).unwrap();
            assert_eq!(
                scalar_path,
                serde_json::json!([
                    {"kind": "type", "value": person_id},
                    {"kind": "field", "value": tag},
                    {"kind": "index", "value": 1},
                ])
            );

            let duplicate_player = || DynamicRolePlayer {
                role_name: "participant".into(),
                player_iid: Some("0xa1".into()),
                player_type_name: Some("person".into()),
                attributes: vec![],
            };
            let role_error = hydrate_relation(
                py,
                package.as_ref(),
                &gathering_id,
                &DynamicRelationRow {
                    iid: Some("0xb1".into()),
                    type_name: Some("gathering".into()),
                    attributes: vec![],
                    role_players: vec![duplicate_player(), duplicate_player()],
                },
            )
            .unwrap_err();
            let role = role_error.value(py);
            assert_eq!(
                role.getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                role.getattr("code").unwrap().extract::<String>().unwrap(),
                "ordered_distinct_duplicate"
            );
            let role_path: serde_json::Value = depythonize(&role.getattr("path").unwrap()).unwrap();
            assert_eq!(
                role_path,
                serde_json::json!([
                    {"kind": "type", "value": gathering_id},
                    {"kind": "role", "value": participant},
                    {"kind": "index", "value": 1},
                    {"kind": "type", "value": person_id},
                ])
            );
        });
    }

    #[test]
    fn ordered_hydration_preserves_inherited_field_role_and_reference_order() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let person = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        ("tag".into(), AttributeValue::String("first".into())),
                        ("tag".into(), AttributeValue::String("second".into())),
                    ],
                },
            )
            .expect("a valid inherited ordered ownership hydrates");
            let values = person.bind(py).call_method0("runtime_values").unwrap();
            let tags = values
                .downcast::<PyDict>()
                .unwrap()
                .get_item("tag")
                .unwrap()
                .unwrap();
            let tags = tags.downcast::<PyTuple>().unwrap();
            let hydrated_tags = tags
                .iter()
                .map(|tag| {
                    tag.call_method0("runtime_attribute_value")
                        .unwrap()
                        .extract::<String>()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(hydrated_tags, ["first", "second"]);

            let player = |iid: &str| DynamicRolePlayer {
                role_name: "participant".into(),
                player_iid: Some(iid.into()),
                player_type_name: Some("person".into()),
                attributes: vec![],
            };
            let gathering = hydrate_relation(
                py,
                package.as_ref(),
                &gathering_id,
                &DynamicRelationRow {
                    iid: Some("0xb1".into()),
                    type_name: Some("gathering".into()),
                    attributes: vec![],
                    role_players: vec![player("0xa1"), player("0xa2")],
                },
            )
            .expect("a valid inherited ordered role hydrates");
            let values = gathering.bind(py).call_method0("runtime_values").unwrap();
            let participants = values
                .downcast::<PyDict>()
                .unwrap()
                .get_item("participant")
                .unwrap()
                .unwrap();
            let participants = participants.downcast::<PyTuple>().unwrap();
            let hydrated_iids = participants
                .iter()
                .map(|participant| {
                    participant
                        .getattr("iid")
                        .unwrap()
                        .extract::<String>()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(hydrated_iids, ["0xa1", "0xa2"]);
        });
    }

    #[test]
    fn ordered_hydration_maps_scalar_constraints_and_iids_to_integrity() {
        use pythonize::depythonize;

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let tag_id = package
                .type_by_label("tag", TypeKind::Attribute)
                .unwrap()
                .clone();

            let scalar_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0xa1".into()),
                    type_name: Some("person".into()),
                    attributes: vec![("tag".into(), AttributeValue::String(String::new()))],
                },
            )
            .unwrap_err();
            let scalar = scalar_error.value(py);
            assert_eq!(
                scalar
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                scalar.getattr("code").unwrap().extract::<String>().unwrap(),
                "regex_constraint_violation"
            );
            let path: serde_json::Value = depythonize(&scalar.getattr("path").unwrap()).unwrap();
            assert_eq!(path, serde_json::json!([{"kind": "type", "value": tag_id}]));

            let iid_error = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("not-a-canonical-iid".into()),
                    type_name: Some("person".into()),
                    attributes: vec![],
                },
            )
            .unwrap_err();
            let iid = iid_error.value(py);
            assert_eq!(
                iid.getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                iid.getattr("code").unwrap().extract::<String>().unwrap(),
                "noncanonical_hydrated_iid"
            );
        });
    }

    #[test]
    fn unordered_hydration_keeps_duplicate_members_on_the_legacy_path() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install(py);
            assert!(
                !PyRuntimeProjection {
                    package: Arc::clone(&package),
                }
                .match_session()
                .unwrap()
                .projected_companion_enabled(),
                "legacy unordered queries must keep the released hydration path",
            );
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let hydrated = hydrate_entity(
                py,
                package.as_ref(),
                &person_id,
                &DynamicEntityRow {
                    iid: Some("0x-person".into()),
                    type_name: Some("person".into()),
                    attributes: vec![
                        (
                            "identifier".into(),
                            AttributeValue::String("person-1".into()),
                        ),
                        ("aliases".into(), AttributeValue::String("same".into())),
                        ("aliases".into(), AttributeValue::String("same".into())),
                    ],
                },
            )
            .expect("legacy unordered hydration accepts repeated collection members");
            let values = hydrated.bind(py).call_method0("runtime_values").unwrap();
            let aliases = values
                .downcast::<PyDict>()
                .unwrap()
                .get_item("aliases")
                .unwrap()
                .unwrap();
            assert_eq!(aliases.downcast::<PyTuple>().unwrap().len(), 2);
        });
    }

    #[test]
    fn ordered_match_sessions_enable_successor_projected_companions() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            assert!(
                PyRuntimeProjection { package }
                    .match_session()
                    .unwrap()
                    .projected_companion_enabled(),
            );
        });
    }

    #[test]
    fn facade_origin_registry_is_exact_identity_weak_and_eagerly_collected() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let values = PyDict::new(py);
            let original =
                hydrate_reference(py, package.as_ref(), &person_id, &values, "0xa1").unwrap();
            let reference = Arc::new(
                ProjectedReference::try_new(
                    package.projection.as_ref(),
                    person_id,
                    Some("0xa1".into()),
                    vec![],
                )
                .unwrap(),
            );
            package
                .facade_origins
                .retain_reference(original.bind(py), Arc::clone(&reference))
                .unwrap();
            assert!(matches!(
                package.facade_origins.proof(original.bind(py)),
                Some(FacadeProjectionProof::Reference(proof)) if proof.as_ref() == reference.as_ref()
            ));

            let copied = py
                .import("copy")
                .unwrap()
                .call_method1("copy", (original.bind(py),))
                .unwrap();
            let lookalike =
                hydrate_reference(py, package.as_ref(), reference.type_id(), &values, "0xa1")
                    .unwrap();
            assert!(package.facade_origins.proof(&copied).is_none());
            assert!(package.facade_origins.proof(lookalike.bind(py)).is_none());

            let observer = PyWeakrefReference::new(original.bind(py)).unwrap().unbind();
            assert_eq!(package.facade_origins.len(), 1);
            drop(original);
            py.import("gc").unwrap().call_method0("collect").unwrap();
            assert!(observer.bind(py).upgrade().is_none());
            assert_eq!(package.facade_origins.len(), 0);
        });
    }

    #[test]
    fn hydrated_facade_origin_is_absent_from_python_introspection_and_serialization() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let (backend, _) = OriginRecordingBackend::new(vec![QueryResult::Documents(vec![
                origin_person_document("0xa0"),
            ])]);
            let database = Arc::new(Database::with_backend(
                Box::new(backend),
                "private-origin-secret",
            ));
            let manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(database),
                None,
                runtime,
            );
            let facade = manager
                .get_by_iid(py, "0xa0")
                .expect("ordered manager hydration should succeed");
            assert!(package.facade_origins.proof(facade.bind(py)).is_some());

            let copied = py
                .import("copy")
                .unwrap()
                .call_method1("copy", (facade.bind(py),))
                .unwrap();
            assert!(package.facade_origins.proof(&copied).is_none());

            let builtins = py.import("builtins").unwrap();
            let names = builtins
                .call_method1("dir", (facade.bind(py),))
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap();
            let forbidden = [
                "origin",
                "origin_carrier",
                "origin_token",
                "database_identity",
                "authority",
            ];
            assert!(names.iter().all(|name| {
                let name = name.to_ascii_lowercase();
                forbidden.iter().all(|needle| !name.contains(needle))
            }));
            for name in forbidden {
                assert!(!facade.bind(py).hasattr(name).unwrap());
            }

            let visible = builtins
                .call_method1("vars", (facade.bind(py),))
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            let mut keys = visible
                .keys()
                .iter()
                .map(|key| key.extract::<String>().unwrap())
                .collect::<Vec<_>>();
            keys.sort();
            assert_eq!(keys, ["_iid", "_values"]);

            let json = py.import("json").unwrap();
            let options = PyDict::new(py);
            options
                .set_item("default", builtins.getattr("vars").unwrap())
                .unwrap();
            options.set_item("sort_keys", true).unwrap();
            let serialized = json
                .call_method("dumps", (facade.bind(py),), Some(&options))
                .unwrap()
                .extract::<String>()
                .unwrap();
            let public_type = facade
                .bind(py)
                .get_type()
                .repr()
                .unwrap()
                .extract::<String>()
                .unwrap();
            let public_mro = facade
                .bind(py)
                .get_type()
                .getattr("__mro__")
                .unwrap()
                .repr()
                .unwrap()
                .extract::<String>()
                .unwrap();
            let representation = facade.bind(py).repr().unwrap().extract::<String>().unwrap();
            for public_text in [serialized, public_type, public_mro, representation] {
                let public_text = public_text.to_ascii_lowercase();
                assert!(!public_text.contains("private-origin-secret"));
                assert!(forbidden.iter().all(|needle| !public_text.contains(needle)));
            }

            let operator = py.import("operator").unwrap();
            assert!(
                operator
                    .call_method1("eq", (facade.bind(py), facade.bind(py)))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            assert!(
                !operator
                    .call_method1("eq", (facade.bind(py), &copied))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
        });
    }

    #[test]
    fn ordered_manager_facades_accept_same_origin_and_fence_foreign_before_io() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let authority = DatabaseConnectionAuthority::isolated();
            let (source_backend, source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xa1")]),
            ]);
            let source = Arc::new(Database::with_backend_authority(
                Box::new(source_backend),
                "shared",
                authority.clone(),
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(source),
                None,
                Arc::clone(&runtime),
            );
            let person = source_manager
                .get_by_iid(py, "0xa1")
                .expect("ordered manager hydration should succeed");
            assert!(package.facade_origins.proof(person.bind(py)).is_some());
            let registry_weakref = py
                .import("weakref")
                .unwrap()
                .call_method1("getweakrefs", (person.bind(py),))
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap()
                .iter()
                .find(|candidate| {
                    candidate
                        .getattr("__callback__")
                        .is_ok_and(|callback| !callback.is_none())
                })
                .expect("the native registry weakref has a cleanup callback");
            registry_weakref
                .getattr("__callback__")
                .unwrap()
                .call1((&registry_weakref,))
                .expect("a hostile live cleanup callback call remains harmless");
            assert!(
                package.facade_origins.proof(person.bind(py)).is_some(),
                "a live caller cannot erase opaque origin proof by invoking the weakref callback",
            );

            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb1"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb1", "0xa1")]),
            ]);
            let same = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same),
                None,
                Arc::clone(&runtime),
            );
            let same_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            same_manager
                .insert(py, same_value)
                .expect("same database authority should accept its hydrated facade");
            {
                let state = same_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (foreign_backend, foreign_state) = OriginRecordingBackend::new(vec![]);
            let foreign = Arc::new(Database::with_backend(Box::new(foreign_backend), "shared"));
            let foreign_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(foreign),
                None,
                runtime,
            );
            let foreign_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            let error = foreign_manager.insert(py, foreign_value).unwrap_err();
            let diagnostic = error.value(py);
            assert_eq!(
                diagnostic
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "invalid_input"
            );
            assert_eq!(
                diagnostic
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            let state = foreign_state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
            assert_eq!(source_state.lock().unwrap().closes, 1);
        });
    }

    #[test]
    fn ordered_borrowed_facade_keeps_transaction_authority_identity() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let authority = DatabaseConnectionAuthority::isolated();
            let (source_backend, source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xa2")]),
            ]);
            let source = Arc::new(Database::with_backend_authority(
                Box::new(source_backend),
                "shared",
                authority.clone(),
            ));
            let transaction = provider_block_on(
                py,
                runtime.as_ref(),
                source.transaction_context(TxType::Read),
            )
            .unwrap();
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                None,
                Some(transaction.clone()),
                Arc::clone(&runtime),
            );
            let person = source_manager
                .get_by_iid(py, "0xa2")
                .expect("borrowed transaction hydration should succeed");

            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb2"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb2", "0xa2")]),
            ]);
            let same = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same),
                None,
                Arc::clone(&runtime),
            );
            let value = gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            same_manager
                .insert(py, value)
                .expect("borrowed proof should share its database authority");
            assert_eq!(same_state.lock().unwrap().commits, 1);

            provider_block_on(py, runtime.as_ref(), transaction.close()).unwrap();
            let state = source_state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Read]);
            assert_eq!(state.queries.len(), 1);
            assert_eq!(state.closes, 1);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn ordered_query_facades_bind_direct_and_borrowed_origins_but_remote_stays_unbound() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));

            let (direct_backend, direct_state) = QueryOriginBackend::new("0xa3");
            let direct_database = Arc::new(Database::with_backend(
                Box::new(direct_backend),
                "query-direct",
            ));
            let (direct_registry, direct_request) = person_query(package.as_ref());
            let direct_result = provider_block_on(
                py,
                runtime.as_ref(),
                direct_database.execute_match(&direct_registry, &direct_request),
            )
            .unwrap();
            let direct_handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::for_database(direct_database.as_ref())),
                direct_request,
                direct_result,
                direct_registry,
                query_budget(),
            )
            .unwrap();
            let direct = hydrate_first_query_thing(py, package.as_ref(), direct_handle);
            let direct_proof = package.facade_origins.proof(direct.bind(py)).unwrap();
            assert!(
                direct_proof
                    .reference(package.projection.as_ref())
                    .unwrap()
                    .origin_carrier()
                    .is_some()
            );

            let (foreign_backend, foreign_state) = OriginRecordingBackend::new(vec![]);
            let foreign = Arc::new(Database::with_backend(
                Box::new(foreign_backend),
                "query-foreign",
            ));
            let foreign_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(foreign),
                None,
                Arc::clone(&runtime),
            );
            let foreign_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, direct.bind(py));
            let error = foreign_manager.insert(py, foreign_value).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = foreign_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = direct_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }

            let authority = DatabaseConnectionAuthority::isolated();
            let (borrowed_backend, borrowed_state) = QueryOriginBackend::new("0xa4");
            let borrowed_database = Arc::new(Database::with_backend_authority(
                Box::new(borrowed_backend),
                "query-shared",
                authority.clone(),
            ));
            let borrowed_transaction = provider_block_on(
                py,
                runtime.as_ref(),
                borrowed_database.transaction_context(TxType::Read),
            )
            .unwrap();
            let (borrowed_registry, borrowed_request) = person_query(package.as_ref());
            let borrowed_result = provider_block_on(
                py,
                runtime.as_ref(),
                borrowed_transaction.execute_match(&borrowed_registry, &borrowed_request),
            )
            .unwrap();
            let borrowed_origin =
                ProjectedQueryOrigin::for_transaction(&borrowed_transaction).unwrap();
            let borrowed_handle = validated_result_handle(
                Some(&package.projection),
                Some(borrowed_origin),
                borrowed_request,
                borrowed_result,
                borrowed_registry,
                query_budget(),
            )
            .unwrap();
            let borrowed = hydrate_first_query_thing(py, package.as_ref(), borrowed_handle);
            let (same_backend, same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb4"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb4", "0xa4")]),
            ]);
            let same_database = Arc::new(Database::with_backend_authority(
                Box::new(same_backend),
                "query-shared",
                authority,
            ));
            let same_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(same_database),
                None,
                Arc::clone(&runtime),
            );
            let same_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, borrowed.bind(py));
            same_manager
                .insert(py, same_value)
                .expect("borrowed query facade should keep database authority identity");
            assert_eq!(same_state.lock().unwrap().commits, 1);
            provider_block_on(py, runtime.as_ref(), borrowed_transaction.close()).unwrap();
            {
                let state = borrowed_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }

            let (remote_backend, remote_state) = QueryOriginBackend::new("0xa5");
            let remote_evidence_database = Arc::new(Database::with_backend(
                Box::new(remote_backend),
                "remote-evidence",
            ));
            let (remote_registry, remote_request) = person_query(package.as_ref());
            let remote_result = provider_block_on(
                py,
                runtime.as_ref(),
                remote_evidence_database.execute_match(&remote_registry, &remote_request),
            )
            .unwrap();
            let remote_handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::remote_unbound()),
                remote_request,
                remote_result,
                remote_registry,
                query_budget(),
            )
            .unwrap();
            let remote = hydrate_first_query_thing(py, package.as_ref(), remote_handle);
            let remote_reference = || {
                package
                    .facade_origins
                    .proof(remote.bind(py))
                    .unwrap()
                    .reference(package.projection.as_ref())
                    .unwrap()
            };
            assert!(remote_reference().origin_carrier().is_none());

            let (target_backend, target_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb5"})]),
                QueryResult::Documents(vec![origin_gathering_document("0xb5", "0xa5")]),
            ]);
            let target = Arc::new(Database::with_backend(
                Box::new(target_backend),
                "remote-target",
            ));
            let target_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(target),
                None,
                runtime,
            );
            let target_value =
                gathering_with_player(py, package.as_ref(), &gathering_id, remote.bind(py));
            target_manager
                .insert(py, target_value)
                .expect("remote query facade must remain explicitly unbound and resolvable");
            assert!(remote_reference().origin_carrier().is_none());
            {
                let state = target_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = remote_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }
        });
    }

    #[test]
    fn query_hydration_rejects_a_mismatched_raw_and_projected_companion() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let runtime = ProviderRuntimeOwner::new().expect("provider runtime should start");

            let (raw_backend, _) = QueryOriginBackend::new("0xae");
            let raw_database = Database::with_backend(Box::new(raw_backend), "mismatch-raw");
            let (raw_registry, raw_request) = person_query(package.as_ref());
            let raw_result = provider_block_on(
                py,
                &runtime,
                raw_database.execute_match(&raw_registry, &raw_request),
            )
            .unwrap();

            let (projected_backend, _) = QueryOriginBackend::new("0xaf");
            let projected_database =
                Database::with_backend(Box::new(projected_backend), "mismatch-projected");
            let (projected_registry, projected_request) = person_query(package.as_ref());
            let projected_result = provider_block_on(
                py,
                &runtime,
                projected_database.execute_match(&projected_registry, &projected_request),
            )
            .unwrap();
            let projected = ProjectedQueryOrigin::remote_unbound()
                .materialize_borrowed_with_budget(
                    package.projection.as_ref(),
                    &projected_registry,
                    &projected_request,
                    &projected_result,
                    ProjectedQueryMaterializationLimits::default(),
                    &AnswerCancellation::default(),
                    None,
                )
                .unwrap()
                .0;

            let handle = crate::validated_result_runtime::PyValidatedMatchResultHandle::new_with_projected_budget(
                raw_request,
                raw_result,
                raw_registry,
                projected,
                QueryExecutionDeadline::for_limits(QueryExecutionResourceLimits::default()),
                AnswerCancellation::default(),
            );
            let handle = Py::new(py, handle).unwrap();
            let row = handle.bind(py).call_method1("row", (0,)).unwrap();
            let slot = row.call_method1("slot", (0,)).unwrap();
            let thing = slot.call_method1("thing", (0,)).unwrap();
            let thing = thing
                .extract::<PyRef<'_, PyValidatedMatchThingHandle>>()
                .unwrap();
            let error = hydrate_validated_thing(py, package.as_ref(), &thing).unwrap_err();
            let diagnostic = error.value(py);
            assert_eq!(
                diagnostic
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "integrity"
            );
            assert_eq!(
                diagnostic
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "projected_query_result_mismatch"
            );
        });
    }

    #[test]
    fn ordered_manager_and_query_relation_players_fence_foreign_origin_before_io() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let container_id = package
                .type_by_label("container", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let manager_authority = DatabaseConnectionAuthority::isolated();

            let (manager_backend, manager_source_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_gathering_document("0xb6", "0xa6")]),
            ]);
            let manager_source = Arc::new(Database::with_backend_authority(
                Box::new(manager_backend),
                "manager-relation-shared",
                manager_authority.clone(),
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(manager_source),
                None,
                Arc::clone(&runtime),
            );
            let manager_relation = source_manager
                .get_by_iid(py, "0xb6")
                .expect("manager relation hydration should succeed");

            let (manager_same_backend, manager_same_state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xb6"})]),
                QueryResult::Documents(vec![serde_json::json!({"iid": "0xc6"})]),
                QueryResult::Documents(vec![origin_container_document("0xc6", "0xb6")]),
            ]);
            let manager_same = Arc::new(Database::with_backend_authority(
                Box::new(manager_same_backend),
                "manager-relation-shared",
                manager_authority,
            ));
            let manager_same_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(manager_same),
                None,
                Arc::clone(&runtime),
            );
            let manager_same_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                manager_relation.bind(py),
            );
            manager_same_manager
                .insert(py, manager_same_container)
                .expect("same authority must accept a hydrated relation-as-player reference");
            {
                let state = manager_same_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 3);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (manager_target_backend, manager_target_state) =
                OriginRecordingBackend::new(vec![]);
            let manager_target = Arc::new(Database::with_backend(
                Box::new(manager_target_backend),
                "manager-relation-target",
            ));
            let manager_target_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(manager_target),
                None,
                Arc::clone(&runtime),
            );
            let manager_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                manager_relation.bind(py),
            );
            let error = manager_target_manager
                .insert(py, manager_container)
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = manager_target_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            assert_eq!(manager_source_state.lock().unwrap().closes, 1);

            let (query_backend, query_source_state) = QueryOriginBackend::gathering("0xb7");
            let query_source = Arc::new(Database::with_backend(
                Box::new(query_backend),
                "query-relation-source",
            ));
            let (registry, request) = thing_query(package.as_ref(), "gathering");
            let result = provider_block_on(
                py,
                runtime.as_ref(),
                query_source.execute_match(&registry, &request),
            )
            .unwrap();
            let handle = validated_result_handle(
                Some(&package.projection),
                Some(ProjectedQueryOrigin::for_database(query_source.as_ref())),
                request,
                result,
                registry,
                query_budget(),
            )
            .unwrap();
            let query_relation = hydrate_first_query_thing(py, package.as_ref(), handle);
            let (query_target_backend, query_target_state) = OriginRecordingBackend::new(vec![]);
            let query_target = Arc::new(Database::with_backend(
                Box::new(query_target_backend),
                "query-relation-target",
            ));
            let query_target_manager = origin_manager(
                Arc::clone(&package),
                container_id.clone(),
                Some(query_target),
                None,
                runtime,
            );
            let query_container = relation_with_player(
                py,
                package.as_ref(),
                &container_id,
                "item",
                query_relation.bind(py),
            );
            let error = query_target_manager
                .insert(py, query_container)
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "reference_database_mismatch"
            );
            {
                let state = query_target_state.lock().unwrap();
                assert!(state.opens.is_empty());
                assert!(state.queries.is_empty());
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            {
                let state = query_source_state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Read]);
                assert_eq!(state.selected, 1);
                assert_eq!(state.hydrated, 1);
                assert_eq!(state.closes, 1);
            }
        });
    }

    #[test]
    fn retained_facade_mutation_is_integrity_failure_before_target_io() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let gathering_id = package
                .type_by_label("gathering", TypeKind::Relation)
                .unwrap()
                .clone();
            let runtime =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            let (source_backend, _) =
                OriginRecordingBackend::new(vec![QueryResult::Documents(vec![
                    origin_person_document("0xa8"),
                ])]);
            let source = Arc::new(Database::with_backend(
                Box::new(source_backend),
                "mutation-source",
            ));
            let source_manager = origin_manager(
                Arc::clone(&package),
                person_id,
                Some(source),
                None,
                Arc::clone(&runtime),
            );
            let person = source_manager.get_by_iid(py, "0xa8").unwrap();
            person.bind(py).setattr("_iid", "0xa9").unwrap();

            let (target_backend, target_state) = OriginRecordingBackend::new(vec![]);
            let target = Arc::new(Database::with_backend(
                Box::new(target_backend),
                "mutation-target",
            ));
            let target_manager = origin_manager(
                Arc::clone(&package),
                gathering_id.clone(),
                Some(target),
                None,
                runtime,
            );
            let value = gathering_with_player(py, package.as_ref(), &gathering_id, person.bind(py));
            let error = target_manager.insert(py, value).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "hydrated_facade_evidence_mismatch"
            );
            let state = target_state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        });
    }

    #[test]
    fn retained_reference_key_mutation_is_not_silently_rebound() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (_, package) = install(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let model = &package.projection.projection().models()[&person_id];
            let key_id = model.reference_read().key_fields()[0].clone();
            let attribute_id = package
                .type_by_label(key_id.attribute().label().as_str(), TypeKind::Attribute)
                .unwrap()
                .clone();
            let key = ProjectedAttributeValue::try_from_attribute_value(
                package.projection.as_ref(),
                attribute_id.clone(),
                &AttributeValue::String("original-key".into()),
            )
            .unwrap();
            let reference = Arc::new(
                ProjectedReference::try_new(
                    package.projection.as_ref(),
                    person_id.clone(),
                    Some("0xaa".into()),
                    vec![(key_id.clone(), key.clone())],
                )
                .unwrap(),
            );
            let fields = BTreeMap::from([(key_id.clone(), vec![key])]);
            let values =
                hydrate_projected_fields(py, package.as_ref(), &person_id, &fields, true).unwrap();
            let facade =
                allocate_projected_reference(py, package.as_ref(), &person_id, &values, "0xaa")
                    .unwrap();
            package
                .facade_origins
                .retain_reference(facade.bind(py), reference)
                .unwrap();

            let changed = package
                .class(&attribute_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py)
                .call1(("changed-key",))
                .unwrap();
            facade
                .bind(py)
                .call_method0("runtime_values")
                .unwrap()
                .downcast::<PyDict>()
                .unwrap()
                .set_item(
                    model.query_tokens().fields()[&key_id]
                        .target_name()
                        .as_str(),
                    changed,
                )
                .unwrap();
            let allowed = BTreeSet::from([ProjectedModelUse::new(
                person_id,
                ProjectedModelForm::Reference,
            )]);
            let error = project_reference(py, package.as_ref(), facade.bind(py), &allowed, &[])
                .unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "hydrated_facade_evidence_mismatch"
            );
        });
    }

    #[test]
    fn ordered_insert_and_put_publish_exact_provider_rehydration() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let tag_id = package
                    .type_by_label("tag", TypeKind::Attribute)
                    .unwrap()
                    .clone();
                let iid = if put { "0xab" } else { "0xaa" };
                let responses = vec![
                    QueryResult::Documents(vec![serde_json::json!({"iid": iid})]),
                    QueryResult::Documents(vec![serde_json::json!({
                        "_iid": iid,
                        "_type": "person",
                        "attributes": {
                            "tag": [{"value": "provider-normalized"}]
                        }
                    })]),
                ];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(Arc::new(Database::with_backend(
                        Box::new(backend),
                        if put {
                            "provider-normalized-put"
                        } else {
                            "provider-normalized-insert"
                        },
                    ))),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let authored = package
                    .class(&tag_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call1(("authored",))
                    .unwrap();
                let kwargs = PyDict::new(py);
                kwargs
                    .set_item("tag", PyTuple::new(py, [authored]).unwrap())
                    .unwrap();
                let instance = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call((), Some(&kwargs))
                    .unwrap();

                let published = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .expect("valid provider rehydration should publish");
                assert!(published.bind(py).is(&instance));
                assert_eq!(
                    instance
                        .getattr("iid")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    iid
                );
                let values = instance
                    .call_method0("runtime_values")
                    .unwrap()
                    .downcast_into::<PyDict>()
                    .unwrap();
                let tags = values
                    .get_item("tag")
                    .unwrap()
                    .unwrap()
                    .downcast_into::<PyTuple>()
                    .unwrap();
                assert_eq!(tags.len(), 1);
                assert_eq!(
                    tags.get_item(0)
                        .unwrap()
                        .call_method0("runtime_attribute_value")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "provider-normalized"
                );

                let proof = match package.facade_origins.proof(&instance).unwrap() {
                    FacadeProjectionProof::Thing(proof) => proof,
                    FacadeProjectionProof::Reference(_) => {
                        panic!("insert/put retained a reference proof")
                    }
                };
                let field_id = package.projection.projection().models()[&person_id]
                    .complete_read()
                    .fields()[0]
                    .token();
                assert_eq!(
                    proof.fields()[field_id][0].to_attribute_value(),
                    AttributeValue::String("provider-normalized".into())
                );
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }
        });
    }

    #[test]
    fn ordered_insert_and_put_reject_malformed_provider_iids_without_publication() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let responses = vec![QueryResult::Documents(vec![serde_json::json!({
                    "iid": "not-a-canonical-iid"
                })])];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let database = Arc::new(Database::with_backend(
                    Box::new(backend),
                    if put {
                        "malformed-put"
                    } else {
                        "malformed-insert"
                    },
                ));
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(database),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let instance = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py)
                    .call0()
                    .unwrap();
                let error = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .unwrap_err();
                let diagnostic = error.value(py);
                assert_eq!(
                    diagnostic
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity"
                );
                assert_eq!(
                    diagnostic
                        .getattr("code")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "provider_hydration_failed"
                );
                assert!(instance.getattr("iid").unwrap().is_none());
                assert!(package.facade_origins.proof(&instance).is_none());
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 1);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 1);
            }
        });
    }

    #[test]
    fn ordered_insert_put_and_update_restore_exact_facade_state_after_local_failure() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let helpers = PyModule::from_code(
                py,
                ffi::c_str!(
                    r#"
def failing_attach(target):
    def attach(self, iid):
        if self is target:
            self._iid = iid
            self._values = {"corrupt": True}
            raise RuntimeError("injected attach failure")
        self._iid = iid
    return attach

def failing_initialize(target, original):
    def initialize(self, values):
        if self is target:
            self._values = {"corrupt": True}
            self._iid = "0xff"
            raise RuntimeError("injected replacement failure")
        return original(self, values)
    return initialize
"#
                ),
                ffi::c_str!("origin_failure_helpers.py"),
                ffi::c_str!("origin_failure_helpers"),
            )
            .unwrap();

            for put in [false, true] {
                let (_, package) = install_ordered(py);
                let person_id = package
                    .type_by_label("person", TypeKind::Entity)
                    .unwrap()
                    .clone();
                let responses = vec![
                    QueryResult::Documents(vec![serde_json::json!({"iid": "0xac"})]),
                    QueryResult::Documents(vec![origin_person_document("0xac")]),
                ];
                let (backend, state) = OriginRecordingBackend::new(responses);
                let manager = origin_manager(
                    Arc::clone(&package),
                    person_id.clone(),
                    Some(Arc::new(Database::with_backend(
                        Box::new(backend),
                        if put {
                            "rollback-put"
                        } else {
                            "rollback-insert"
                        },
                    ))),
                    None,
                    Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
                );
                let class = package
                    .class(&person_id, ProjectedModelForm::Complete)
                    .unwrap()
                    .bind(py);
                let instance = class.call0().unwrap();
                let prior_values = instance.getattr("_values").unwrap().unbind();
                let prior_iid = instance.getattr("_iid").unwrap().unbind();
                let original_attach = class.getattr("attach_runtime_iid").unwrap().unbind();
                let failing = helpers
                    .getattr("failing_attach")
                    .unwrap()
                    .call1((&instance,))
                    .unwrap();
                class.setattr("attach_runtime_iid", failing).unwrap();

                let error = if put {
                    manager.put(py, instance.clone())
                } else {
                    manager.insert(py, instance.clone())
                }
                .unwrap_err();
                class
                    .setattr("attach_runtime_iid", original_attach.bind(py))
                    .unwrap();
                assert!(error.to_string().contains("injected attach failure"));
                assert!(
                    instance
                        .getattr("_values")
                        .unwrap()
                        .is(prior_values.bind(py))
                );
                assert!(instance.getattr("_iid").unwrap().is(prior_iid.bind(py)));
                assert!(package.facade_origins.proof(&instance).is_none());
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 2);
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
            }

            let (_, package) = install_ordered(py);
            let person_id = package
                .type_by_label("person", TypeKind::Entity)
                .unwrap()
                .clone();
            let (backend, state) = OriginRecordingBackend::new(vec![
                QueryResult::Documents(vec![origin_person_document("0xad")]),
                QueryResult::Ok,
                QueryResult::Documents(vec![origin_person_document("0xad")]),
            ]);
            let manager = origin_manager(
                Arc::clone(&package),
                person_id.clone(),
                Some(Arc::new(Database::with_backend(
                    Box::new(backend),
                    "rollback-update",
                ))),
                None,
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start")),
            );
            let instance = manager.get_by_iid(py, "0xad").unwrap();
            let prior_values = instance.bind(py).getattr("_values").unwrap().unbind();
            let prior_iid = instance.bind(py).getattr("_iid").unwrap().unbind();
            let prior_proof = match package.facade_origins.proof(instance.bind(py)).unwrap() {
                FacadeProjectionProof::Thing(proof) => proof,
                FacadeProjectionProof::Reference(_) => panic!("manager get retained a reference"),
            };
            let class = package
                .class(&person_id, ProjectedModelForm::Complete)
                .unwrap()
                .bind(py);
            let original_initialize = class.getattr("initialize_runtime_values").unwrap().unbind();
            let failing = helpers
                .getattr("failing_initialize")
                .unwrap()
                .call1((instance.bind(py), original_initialize.bind(py)))
                .unwrap();
            class.setattr("initialize_runtime_values", failing).unwrap();
            let error = manager.update(py, instance.bind(py).clone()).unwrap_err();
            class
                .setattr("initialize_runtime_values", original_initialize.bind(py))
                .unwrap();
            assert!(error.to_string().contains("injected replacement failure"));
            assert!(
                instance
                    .bind(py)
                    .getattr("_values")
                    .unwrap()
                    .is(prior_values.bind(py))
            );
            assert!(
                instance
                    .bind(py)
                    .getattr("_iid")
                    .unwrap()
                    .is(prior_iid.bind(py))
            );
            let after_proof = match package.facade_origins.proof(instance.bind(py)).unwrap() {
                FacadeProjectionProof::Thing(proof) => proof,
                FacadeProjectionProof::Reference(_) => panic!("update retained a reference"),
            };
            assert!(Arc::ptr_eq(&prior_proof, &after_proof));
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Read, TxType::Write]);
            assert_eq!(state.queries.len(), 3);
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
            assert_eq!(state.closes, 1);
        });
    }

    #[test]
    fn ordered_install_normalizes_all_projection_admission_failures() {
        use pythonize::depythonize;

        let ordered_authority = authority(ORDERED_SCHEMA, "python-ordered.yaml");
        let authority_bytes = encode_schema_authority(&ordered_authority);
        let exact = python_projection(&ordered_authority);
        let emitter = PythonEmitter::new();
        assert_eq!(exact.generator_handlers(), [ProjectionHandler::python_v2()]);

        let rejected = |py: Python<'_>,
                        projection_json: &str,
                        semantic_fingerprint_json: &str,
                        projection_fingerprint_json: &str,
                        authority_bytes: Option<&[u8]>| {
            install_projection(
                py,
                projection_json,
                semantic_fingerprint_json,
                projection_fingerprint_json,
                vec![],
                authority_bytes,
            )
            .err()
            .expect("hostile ordered projection evidence must fail")
        };

        let mut missing_resources = exact.code_resources().to_vec();
        missing_resources.pop();
        let missing = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &missing_resources,
        )
        .unwrap();

        let mut extra_resources = exact.code_resources().to_vec();
        extra_resources.push(
            CodeResourceDigest::from_bytes(
                "typebridge.generator.python.unexpected-resource",
                b"unexpected Python resource",
            )
            .unwrap(),
        );
        extra_resources.sort_by(|left, right| left.id().cmp(right.id()));
        let extra = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &extra_resources,
        )
        .unwrap();

        let stale = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();

        let mut forged_resources = exact.code_resources().to_vec();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged Python resource").unwrap();
        let forged = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &forged_resources,
        )
        .unwrap();

        let exact_json = String::from_utf8(to_canonical_json(&exact).unwrap()).unwrap();
        let exact_semantic =
            String::from_utf8(to_canonical_json(exact.semantic_fingerprint()).unwrap()).unwrap();
        let exact_fingerprint =
            String::from_utf8(to_canonical_json(exact.projection_fingerprint()).unwrap()).unwrap();

        let projection_parts = |projection: &RuntimeProjection| {
            (
                String::from_utf8(to_canonical_json(projection).unwrap()).unwrap(),
                String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                    .unwrap(),
                String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                    .unwrap(),
            )
        };
        let missing = projection_parts(&missing);
        let extra = projection_parts(&extra);
        let stale = projection_parts(&stale);
        let forged = projection_parts(&forged);

        let mut duplicate: serde_json::Value = serde_json::from_str(&exact_json).unwrap();
        let resources = duplicate["code_resources"].as_array_mut().unwrap();
        let repeated_resource = resources[0].clone();
        resources.insert(1, repeated_resource);
        let duplicate = String::from_utf8(to_canonical_json(&duplicate).unwrap()).unwrap();

        let mut reordered: serde_json::Value = serde_json::from_str(&exact_json).unwrap();
        reordered["code_resources"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let reordered = String::from_utf8(to_canonical_json(&reordered).unwrap()).unwrap();

        let noncanonical = serde_json::to_string_pretty(
            &serde_json::from_str::<serde_json::Value>(&exact_json).unwrap(),
        )
        .unwrap();

        let typescript = TypeScriptEmitter::new();
        let foreign_target = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &typescript.generator_handlers_for(ordered_authority.resolved_schema()),
            &typescript
                .code_resources_for(ordered_authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let foreign_target = projection_parts(&foreign_target);

        let foreign = authority(
            "format: typebridge.schema/v2\nentities:\n  foreign: {}\n",
            "python-foreign.yaml",
        );
        let foreign = encode_schema_authority(&foreign);

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            install_projection(
                py,
                &exact_json,
                &exact_semantic,
                &exact_fingerprint,
                classes(py, &exact),
                Some(&authority_bytes),
            )
            .expect("the exact ordered Python package evidence must install");

            for error in [
                rejected(py, &exact_json, &exact_semantic, &exact_fingerprint, None),
                rejected(
                    py,
                    &missing.0,
                    &missing.1,
                    &missing.2,
                    Some(&authority_bytes),
                ),
                rejected(py, &extra.0, &extra.1, &extra.2, Some(&authority_bytes)),
                rejected(
                    py,
                    &duplicate,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &reordered,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(py, &forged.0, &forged.1, &forged.2, Some(&authority_bytes)),
                rejected(py, &stale.0, &stale.1, &stale.2, Some(&authority_bytes)),
                rejected(
                    py,
                    &noncanonical,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    "{",
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    "{",
                    &exact_fingerprint,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    "{",
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &foreign_target.0,
                    &foreign_target.1,
                    &foreign_target.2,
                    Some(&authority_bytes),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(b"{"),
                ),
                rejected(
                    py,
                    &exact_json,
                    &exact_semantic,
                    &exact_fingerprint,
                    Some(&foreign),
                ),
            ] {
                let value = error.value(py);
                assert_eq!(
                    value
                        .getattr("category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity",
                );
                assert_eq!(
                    value
                        .getattr("sdk_category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "integrity",
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    "projection_evidence_mismatch",
                );
                assert_eq!(
                    value
                        .getattr("message")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    "Generated projection evidence does not match the verified schema package",
                );
                let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
                assert_eq!(
                    path,
                    serde_json::json!([
                        {"kind": "argument", "value": "projection_evidence"}
                    ]),
                );
            }
        });
    }

    #[test]
    fn authorityless_unordered_install_keeps_legacy_diagnostics() {
        let unordered_authority = authority(SCHEMA, "python-legacy-diagnostics.yaml");
        let projection = python_projection(&unordered_authority);
        let projection_json = String::from_utf8(to_canonical_json(&projection).unwrap()).unwrap();
        let semantic =
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap();
        let fingerprint =
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap();

        let typescript = TypeScriptEmitter::new();
        let foreign_target = project(
            unordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &typescript.generator_handlers(),
            &typescript.code_resources().unwrap(),
        )
        .unwrap();
        let foreign_json = String::from_utf8(to_canonical_json(&foreign_target).unwrap()).unwrap();
        let foreign_semantic =
            String::from_utf8(to_canonical_json(foreign_target.semantic_fingerprint()).unwrap())
                .unwrap();
        let foreign_fingerprint =
            String::from_utf8(to_canonical_json(foreign_target.projection_fingerprint()).unwrap())
                .unwrap();

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let malformed = install_projection(py, "{", &semantic, &fingerprint, vec![], None)
                .err()
                .expect("malformed legacy projection must fail");
            let malformed_value = malformed.value(py);
            assert!(malformed_value.is_instance_of::<pyo3::exceptions::PyValueError>());
            assert_eq!(
                malformed_value.str().unwrap().to_str().unwrap(),
                "invalid_contract [malformed_canonical_json]: input is not valid canonical JSON",
            );
            assert!(malformed_value.getattr("sdk_category").is_err());

            let target = install_projection(
                py,
                &foreign_json,
                &foreign_semantic,
                &foreign_fingerprint,
                vec![],
                None,
            )
            .err()
            .expect("foreign legacy target must fail");
            let target_value = target.value(py);
            assert!(target_value.is_instance_of::<pyo3::exceptions::PyRuntimeError>());
            assert_eq!(
                target_value.str().unwrap().to_str().unwrap(),
                "runtime projection does not target Python",
            );
            assert!(target_value.getattr("sdk_category").is_err());

            let coverage =
                install_projection(py, &projection_json, &semantic, &fingerprint, vec![], None)
                    .err()
                    .expect("incomplete legacy registrations must fail");
            let coverage_value = coverage.value(py);
            assert!(coverage_value.is_instance_of::<pyo3::exceptions::PyValueError>());
            assert_eq!(
                coverage_value.str().unwrap().to_str().unwrap(),
                format!(
                    "projection requires exactly {} model registrations, received 0",
                    projection.models().len(),
                ),
            );
            assert!(coverage_value.getattr("sdk_category").is_err());
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
