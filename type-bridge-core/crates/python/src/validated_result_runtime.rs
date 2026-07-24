//! Fine-grained PyO3 views over one exact-invocation validated match result.
//!
//! No class in this module is Python-constructible. Every child view retains
//! the same proof owner and rechecks request-token and shape lineage before it
//! exposes one row, slot, thing, attribute, role, or role player.

use std::sync::Arc;

use pyo3::exceptions::{PyIndexError, PyRuntimeError};
use pyo3::prelude::*;
use pythonize::pythonize;
use type_bridge_orm::{
    AttributeValue, DescriptorId, DescriptorRegistry, FetchShape, HydratedAttribute, HydratedRole,
    HydratedRolePlayer, HydratedThing, MatchOperation, MatchResult, MatchRow, SlotValue, ThingKind,
    ValidatedMatchRequest, ValidatedMatchResult, Window,
};

use crate::match_runtime::py_match_error;

struct ValidatedResultProof {
    request: ValidatedMatchRequest,
    result: ValidatedMatchResult,
    registry: Arc<DescriptorRegistry>,
}

impl ValidatedResultProof {
    fn result(&self) -> PyResult<&MatchResult> {
        self.result
            .for_request(&self.request)
            .map_err(py_match_error)
    }

    fn rows(&self) -> PyResult<&[MatchRow]> {
        match (&self.request.request().operation, self.result()?) {
            (MatchOperation::FetchRows { .. }, MatchResult::Rows { rows }) => Ok(rows),
            _ => Err(access_error(
                "validated selected-row handle contains a non-row result",
            )),
        }
    }

    fn page(&self) -> PyResult<(&[MatchRow], Window, Option<u64>)> {
        match (&self.request.request().operation, self.result()?) {
            (
                MatchOperation::PageBy {
                    root: expected_root,
                    window: expected_window,
                    include_total,
                    ..
                },
                MatchResult::Page {
                    root,
                    entries,
                    window,
                    total,
                },
            ) => {
                if root != expected_root || window != expected_window {
                    return Err(access_error(
                        "validated page result no longer matches its operation root and window",
                    ));
                }
                if *include_total != total.is_some() {
                    return Err(access_error(
                        "validated page total presence no longer matches its operation",
                    ));
                }
                Ok((entries, *window, *total))
            }
            _ => Err(access_error(
                "validated page handle contains a non-page result",
            )),
        }
    }

    fn count(&self) -> PyResult<u64> {
        match (&self.request.request().operation, self.result()?) {
            (
                MatchOperation::CountBy {
                    root: expected_root,
                },
                MatchResult::Count { root, value },
            ) if root == expected_root => Ok(*value),
            _ => Err(access_error(
                "validated count handle contains a non-count or foreign-root result",
            )),
        }
    }

    fn exists(&self) -> PyResult<bool> {
        match (&self.request.request().operation, self.result()?) {
            (
                MatchOperation::ExistsBy {
                    root: expected_root,
                },
                MatchResult::Exists { root, value },
            ) if root == expected_root => Ok(*value),
            _ => Err(access_error(
                "validated exists handle contains a non-exists or foreign-root result",
            )),
        }
    }

    fn shape(&self) -> PyResult<&FetchShape> {
        match &self.request.request().operation {
            MatchOperation::FetchRows { output, .. } | MatchOperation::PageBy { output, .. } => {
                Ok(output)
            }
            _ => Err(access_error(
                "validated selected output handle contains a scalar request",
            )),
        }
    }

    fn selected_rows(&self) -> PyResult<&[MatchRow]> {
        match &self.request.request().operation {
            MatchOperation::FetchRows { .. } => self.rows(),
            MatchOperation::PageBy { .. } => self.page().map(|(entries, _, _)| entries),
            _ => Err(access_error(
                "validated selected output handle contains a scalar result",
            )),
        }
    }

    fn selected_row(&self, index: usize) -> PyResult<&MatchRow> {
        self.selected_rows()?
            .get(index)
            .ok_or_else(|| index_error("row", index))
    }

    fn slot(&self, path: SlotPath) -> PyResult<&SlotValue> {
        let slot = self
            .selected_row(path.row)?
            .slots()
            .get(path.slot)
            .ok_or_else(|| index_error("slot", path.slot))?;
        let expected_collection = match self.shape()? {
            FetchShape::Positional { slots } => slots
                .get(path.slot)
                .ok_or_else(|| index_error("shape slot", path.slot))?
                .is_collection(),
            FetchShape::Named { slots } => slots
                .get(path.slot)
                .ok_or_else(|| index_error("shape slot", path.slot))?
                .slot
                .is_collection(),
        };
        if expected_collection != matches!(slot, SlotValue::Many(_)) {
            return Err(access_error(
                "validated result slot cardinality no longer matches its request",
            ));
        }
        Ok(slot)
    }

    fn slot_name(&self, index: usize) -> PyResult<Option<String>> {
        match self.shape()? {
            FetchShape::Positional { slots } => {
                slots
                    .get(index)
                    .ok_or_else(|| index_error("shape slot", index))?;
                Ok(None)
            }
            FetchShape::Named { slots } => Ok(Some(
                slots
                    .get(index)
                    .ok_or_else(|| index_error("shape slot", index))?
                    .name
                    .clone(),
            )),
        }
    }

    fn thing(&self, path: ThingPath) -> PyResult<&HydratedThing> {
        match self.slot(path.slot)? {
            SlotValue::One(thing) if path.thing == 0 => Ok(thing),
            SlotValue::One(_) => Err(index_error("singular slot thing", path.thing)),
            SlotValue::Many(things) => things
                .get(path.thing)
                .ok_or_else(|| index_error("collection thing", path.thing)),
        }
    }

    fn role(&self, path: RolePath) -> PyResult<&HydratedRole> {
        self.thing(path.thing)?
            .roles()
            .get(path.role)
            .ok_or_else(|| index_error("role", path.role))
    }

    fn role_player(&self, path: RolePlayerPath) -> PyResult<&HydratedRolePlayer> {
        self.role(path.role)?
            .players()
            .get(path.player)
            .ok_or_else(|| index_error("role player", path.player))
    }

    fn attribute(&self, path: AttributePath) -> PyResult<&HydratedAttribute> {
        let attributes = match path.owner {
            AttributeOwnerPath::Thing(thing) => self.thing(thing)?.attributes(),
            AttributeOwnerPath::RolePlayer(player) => self.role_player(player)?.attributes(),
        };
        attributes
            .get(path.attribute)
            .ok_or_else(|| index_error("attribute", path.attribute))
    }

    fn descriptor_type_name(&self, descriptor: &DescriptorId) -> PyResult<String> {
        self.registry
            .descriptor_type_name(descriptor)
            .ok_or_else(|| access_error("validated result descriptor is no longer registered"))
    }
}

#[derive(Clone, Copy)]
struct SlotPath {
    row: usize,
    slot: usize,
}

#[derive(Clone, Copy)]
struct ThingPath {
    slot: SlotPath,
    thing: usize,
}

#[derive(Clone, Copy)]
struct RolePath {
    thing: ThingPath,
    role: usize,
}

#[derive(Clone, Copy)]
struct RolePlayerPath {
    role: RolePath,
    player: usize,
}

#[derive(Clone, Copy)]
enum AttributeOwnerPath {
    Thing(ThingPath),
    RolePlayer(RolePlayerPath),
}

#[derive(Clone, Copy)]
struct AttributePath {
    owner: AttributeOwnerPath,
    attribute: usize,
}

#[pyclass(name = "ValidatedMatchResultHandle", frozen)]
#[derive(Clone)]
pub(crate) struct PyValidatedMatchResultHandle {
    proof: Arc<ValidatedResultProof>,
}

impl PyValidatedMatchResultHandle {
    pub(crate) fn new(
        request: ValidatedMatchRequest,
        result: ValidatedMatchResult,
        registry: Arc<DescriptorRegistry>,
    ) -> Self {
        Self {
            proof: Arc::new(ValidatedResultProof {
                request,
                result,
                registry,
            }),
        }
    }
}

#[pymethods]
impl PyValidatedMatchResultHandle {
    fn row_count(&self) -> PyResult<usize> {
        Ok(self.proof.rows()?.len())
    }

    fn row(&self, index: usize) -> PyResult<PyValidatedMatchRowHandle> {
        self.proof.rows()?;
        self.proof.selected_row(index)?;
        Ok(PyValidatedMatchRowHandle {
            proof: Arc::clone(&self.proof),
            row: index,
        })
    }

    fn page_entry_count(&self) -> PyResult<usize> {
        Ok(self.proof.page()?.0.len())
    }

    fn page_entry(&self, index: usize) -> PyResult<PyValidatedMatchRowHandle> {
        self.proof.page()?;
        self.proof.selected_row(index)?;
        Ok(PyValidatedMatchRowHandle {
            proof: Arc::clone(&self.proof),
            row: index,
        })
    }

    fn page_offset(&self) -> PyResult<u64> {
        Ok(self.proof.page()?.1.offset)
    }

    fn page_limit(&self) -> PyResult<u64> {
        Ok(self.proof.page()?.1.limit)
    }

    fn page_total(&self) -> PyResult<Option<u64>> {
        Ok(self.proof.page()?.2)
    }

    fn count_value(&self) -> PyResult<u64> {
        self.proof.count()
    }

    fn exists_value(&self) -> PyResult<bool> {
        self.proof.exists()
    }
}

#[pyclass(name = "ValidatedMatchRowHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchRowHandle {
    proof: Arc<ValidatedResultProof>,
    row: usize,
}

#[pymethods]
impl PyValidatedMatchRowHandle {
    fn slot_count(&self) -> PyResult<usize> {
        Ok(self.proof.selected_row(self.row)?.slots().len())
    }

    fn slot(&self, index: usize) -> PyResult<PyValidatedMatchSlotHandle> {
        let path = SlotPath {
            row: self.row,
            slot: index,
        };
        self.proof.slot(path)?;
        Ok(PyValidatedMatchSlotHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }
}

#[pyclass(name = "ValidatedMatchSlotHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchSlotHandle {
    proof: Arc<ValidatedResultProof>,
    path: SlotPath,
}

#[pymethods]
impl PyValidatedMatchSlotHandle {
    fn name(&self) -> PyResult<Option<String>> {
        self.proof.slot(self.path)?;
        self.proof.slot_name(self.path.slot)
    }

    fn is_collection(&self) -> PyResult<bool> {
        Ok(matches!(self.proof.slot(self.path)?, SlotValue::Many(_)))
    }

    fn thing_count(&self) -> PyResult<usize> {
        match self.proof.slot(self.path)? {
            SlotValue::One(_) => Ok(1),
            SlotValue::Many(things) => Ok(things.len()),
        }
    }

    fn thing(&self, index: usize) -> PyResult<PyValidatedMatchThingHandle> {
        let path = ThingPath {
            slot: self.path,
            thing: index,
        };
        self.proof.thing(path)?;
        Ok(PyValidatedMatchThingHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }
}

#[pyclass(name = "ValidatedMatchThingHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchThingHandle {
    proof: Arc<ValidatedResultProof>,
    path: ThingPath,
}

#[pymethods]
impl PyValidatedMatchThingHandle {
    fn iid(&self) -> PyResult<String> {
        Ok(self
            .proof
            .thing(self.path)?
            .concept_id()
            .as_str()
            .to_owned())
    }

    fn declared_type_name(&self) -> PyResult<String> {
        self.proof
            .descriptor_type_name(self.proof.thing(self.path)?.declared_descriptor())
    }

    fn concrete_type_name(&self) -> PyResult<String> {
        self.proof
            .descriptor_type_name(self.proof.thing(self.path)?.concrete_descriptor())
    }

    fn kind(&self) -> PyResult<&'static str> {
        Ok(kind_name(self.proof.thing(self.path)?.kind()))
    }

    fn attribute_count(&self) -> PyResult<usize> {
        Ok(self.proof.thing(self.path)?.attributes().len())
    }

    fn attribute(&self, index: usize) -> PyResult<PyValidatedMatchAttributeHandle> {
        let path = AttributePath {
            owner: AttributeOwnerPath::Thing(self.path),
            attribute: index,
        };
        self.proof.attribute(path)?;
        Ok(PyValidatedMatchAttributeHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }

    fn role_count(&self) -> PyResult<usize> {
        Ok(self.proof.thing(self.path)?.roles().len())
    }

    fn role(&self, index: usize) -> PyResult<PyValidatedMatchRoleHandle> {
        let path = RolePath {
            thing: self.path,
            role: index,
        };
        self.proof.role(path)?;
        Ok(PyValidatedMatchRoleHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }
}

#[pyclass(name = "ValidatedMatchAttributeHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchAttributeHandle {
    proof: Arc<ValidatedResultProof>,
    path: AttributePath,
}

#[pymethods]
impl PyValidatedMatchAttributeHandle {
    fn field_name(&self) -> PyResult<String> {
        Ok(self.proof.attribute(self.path)?.field().name.clone())
    }

    fn value_count(&self) -> PyResult<usize> {
        Ok(self.proof.attribute(self.path)?.values().len())
    }

    fn value_type(&self, index: usize) -> PyResult<&'static str> {
        Ok(self
            .proof
            .attribute(self.path)?
            .values()
            .get(index)
            .ok_or_else(|| index_error("attribute value", index))?
            .value_type_name())
    }

    fn value(&self, py: Python<'_>, index: usize) -> PyResult<PyObject> {
        let value = self
            .proof
            .attribute(self.path)?
            .values()
            .get(index)
            .ok_or_else(|| index_error("attribute value", index))?;
        attribute_value_to_py(py, value)
    }
}

#[pyclass(name = "ValidatedMatchRoleHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchRoleHandle {
    proof: Arc<ValidatedResultProof>,
    path: RolePath,
}

#[pymethods]
impl PyValidatedMatchRoleHandle {
    fn role_name(&self) -> PyResult<String> {
        Ok(self.proof.role(self.path)?.role().name.clone())
    }

    fn player_count(&self) -> PyResult<usize> {
        Ok(self.proof.role(self.path)?.players().len())
    }

    fn player(&self, index: usize) -> PyResult<PyValidatedMatchRolePlayerHandle> {
        let path = RolePlayerPath {
            role: self.path,
            player: index,
        };
        self.proof.role_player(path)?;
        Ok(PyValidatedMatchRolePlayerHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }
}

#[pyclass(name = "ValidatedMatchRolePlayerHandle", frozen)]
#[derive(Clone)]
struct PyValidatedMatchRolePlayerHandle {
    proof: Arc<ValidatedResultProof>,
    path: RolePlayerPath,
}

#[pymethods]
impl PyValidatedMatchRolePlayerHandle {
    fn iid(&self) -> PyResult<String> {
        Ok(self
            .proof
            .role_player(self.path)?
            .concept_id()
            .as_str()
            .to_owned())
    }

    fn declared_type_name(&self) -> PyResult<String> {
        self.proof
            .descriptor_type_name(self.proof.role_player(self.path)?.declared_descriptor())
    }

    fn concrete_type_name(&self) -> PyResult<String> {
        self.proof
            .descriptor_type_name(self.proof.role_player(self.path)?.concrete_descriptor())
    }

    fn kind(&self) -> PyResult<&'static str> {
        Ok(kind_name(self.proof.role_player(self.path)?.kind()))
    }

    fn attribute_count(&self) -> PyResult<usize> {
        Ok(self.proof.role_player(self.path)?.attributes().len())
    }

    fn attribute(&self, index: usize) -> PyResult<PyValidatedMatchAttributeHandle> {
        let path = AttributePath {
            owner: AttributeOwnerPath::RolePlayer(self.path),
            attribute: index,
        };
        self.proof.attribute(path)?;
        Ok(PyValidatedMatchAttributeHandle {
            proof: Arc::clone(&self.proof),
            path,
        })
    }
}

fn kind_name(kind: ThingKind) -> &'static str {
    match kind {
        ThingKind::Entity => "entity",
        ThingKind::Relation => "relation",
    }
}

fn attribute_value_to_py(py: Python<'_>, value: &AttributeValue) -> PyResult<PyObject> {
    let value = match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => pythonize(py, value),
        AttributeValue::Long(value) => pythonize(py, value),
        AttributeValue::Double(value) => pythonize(py, value),
        AttributeValue::Boolean(value) => pythonize(py, value),
    }
    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    Ok(value.unbind())
}

fn access_error(message: &'static str) -> PyErr {
    PyRuntimeError::new_err(message)
}

fn index_error(kind: &'static str, index: usize) -> PyErr {
    PyIndexError::new_err(format!("validated {kind} index {index} is out of range"))
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyValidatedMatchResultHandle>()?;
    module.add_class::<PyValidatedMatchRowHandle>()?;
    module.add_class::<PyValidatedMatchSlotHandle>()?;
    module.add_class::<PyValidatedMatchThingHandle>()?;
    module.add_class::<PyValidatedMatchAttributeHandle>()?;
    module.add_class::<PyValidatedMatchRoleHandle>()?;
    module.add_class::<PyValidatedMatchRolePlayerHandle>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use type_bridge_core_lib::ast::{
        TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
    };
    use type_bridge_orm::session::backend::{
        AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
        BoundedAnswerStats, BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
    };
    use type_bridge_orm::{
        Annotation, Database, DescriptorRegistry, EntityDescriptor, OrmError,
        OwnedAttributeDescriptor, RowCardinality, SessionHandle, ValueType, Window,
    };

    use super::*;

    struct AccessorBackend;

    impl DriverBackend for AccessorBackend {
        fn match_capabilities(&self) -> type_bridge_orm::CapabilitySet {
            type_bridge_orm::CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Ok(Box::new(AccessorTransaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct AccessorTransaction;

    impl TransactionOps for AccessorTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("validated accessor test used a legacy string query") })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move {
                feed(
                    vec![AnswerItem::Row(serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    }))],
                    limits,
                    consumer,
                )
            })
        }

        fn query_tuple_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move {
                feed(
                    vec![AnswerItem::Row(serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    }))],
                    limits,
                    consumer,
                )
            })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move {
                feed(
                    vec![AnswerItem::Document(serde_json::json!({
                        "binding": 0,
                        "concept_id": "0x01",
                        "concrete_type": "person",
                        "kind": "entity",
                        "attributes": [{
                            "field": "name",
                            "value_type": "string",
                            "values": ["Alice"],
                        }],
                        "roles": [],
                    }))],
                    limits,
                    consumer,
                )
            })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move {
                feed(
                    vec![AnswerItem::Row(serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    }))],
                    limits,
                    consumer,
                )
            })
        }

        fn rematch_page_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedPageRematch,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            Box::pin(async move {
                feed(
                    vec![AnswerItem::Document(serde_json::json!({
                        "bindings": [{
                            "binding": 0,
                            "concept_id": "0x01",
                            "concrete_type": "person",
                            "kind": "entity",
                            "attributes": [{
                                "field": "name",
                                "value_type": "string",
                                "values": ["Alice"],
                            }],
                            "roles": [],
                        }],
                        "satisfied_role_edges": [],
                    }))],
                    limits,
                    consumer,
                )
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn feed(
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

    fn registry() -> Arc<DescriptorRegistry> {
        let registry = Arc::new(DescriptorRegistry::new());
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "person-name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn executed_handle() -> PyValidatedMatchResultHandle {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
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
        let database = Database::with_backend(Box::new(AccessorBackend), "test");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime
            .block_on(database.execute_match(&registry, &validated))
            .unwrap();
        PyValidatedMatchResultHandle::new(validated, result, registry)
    }

    fn executed_page_handle() -> PyValidatedMatchResultHandle {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let query = session.query(shape).unwrap();
        let validated = query
            .validate_page_by(
                &person,
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                false,
            )
            .unwrap();
        execute_handle(registry, validated)
    }

    fn executed_count_handle() -> PyValidatedMatchResultHandle {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let query = session.query(shape).unwrap();
        let validated = query.validate_count_by(&person).unwrap();
        execute_handle(registry, validated)
    }

    fn executed_exists_handle() -> PyValidatedMatchResultHandle {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let query = session.query(shape).unwrap();
        let validated = query.validate_exists_by(&person).unwrap();
        execute_handle(registry, validated)
    }

    fn execute_handle(
        registry: Arc<DescriptorRegistry>,
        validated: ValidatedMatchRequest,
    ) -> PyValidatedMatchResultHandle {
        let database = Database::with_backend(Box::new(AccessorBackend), "test");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime
            .block_on(database.execute_match(&registry, &validated))
            .unwrap();
        PyValidatedMatchResultHandle::new(validated, result, registry)
    }

    #[test]
    fn exact_lineage_handle_exposes_only_fine_grained_validated_accessors() {
        let handle = executed_handle();
        assert_eq!(handle.row_count().unwrap(), 1);
        let row = handle.row(0).unwrap();
        assert_eq!(row.slot_count().unwrap(), 1);
        let slot = row.slot(0).unwrap();
        assert_eq!(slot.name().unwrap(), None);
        assert!(!slot.is_collection().unwrap());
        assert_eq!(slot.thing_count().unwrap(), 1);
        let thing = slot.thing(0).unwrap();
        assert_eq!(thing.iid().unwrap(), "0x01");
        assert_eq!(thing.declared_type_name().unwrap(), "person");
        assert_eq!(thing.concrete_type_name().unwrap(), "person");
        assert_eq!(thing.kind().unwrap(), "entity");
        assert_eq!(thing.attribute_count().unwrap(), 1);
        assert_eq!(thing.role_count().unwrap(), 0);
        let attribute = thing.attribute(0).unwrap();
        assert_eq!(attribute.field_name().unwrap(), "name");
        assert_eq!(attribute.value_count().unwrap(), 1);
        assert_eq!(attribute.value_type(0).unwrap(), "string");

        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            assert_eq!(
                attribute
                    .value(py, 0)
                    .unwrap()
                    .extract::<String>(py)
                    .unwrap(),
                "Alice"
            );
        });
    }

    #[test]
    fn registered_accessor_types_are_nonconstructible_and_frozen() {
        let handle = executed_handle();
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let module = PyModule::new(py, "validated_result_test").unwrap();
            register(&module).unwrap();
            for name in [
                "ValidatedMatchResultHandle",
                "ValidatedMatchRowHandle",
                "ValidatedMatchSlotHandle",
                "ValidatedMatchThingHandle",
                "ValidatedMatchAttributeHandle",
                "ValidatedMatchRoleHandle",
                "ValidatedMatchRolePlayerHandle",
            ] {
                assert!(module.getattr(name).unwrap().call0().is_err(), "{name}");
            }

            let object = Py::new(py, handle).unwrap();
            let object = object.bind(py);
            assert!(object.getattr("__dict__").is_err());
            assert!(object.setattr("proof", py.None()).is_err());
            assert!(!object.hasattr("result").unwrap());
            assert!(!object.hasattr("request_token").unwrap());
        });
    }

    #[test]
    fn page_count_and_exists_accessors_are_operation_specific_and_lossless() {
        let page = executed_page_handle();
        assert_eq!(page.page_entry_count().unwrap(), 1);
        assert_eq!(page.page_entry(0).unwrap().slot_count().unwrap(), 1);
        assert_eq!(page.page_offset().unwrap(), 0);
        assert_eq!(page.page_limit().unwrap(), 1);
        assert_eq!(page.page_total().unwrap(), None);
        assert!(page.row_count().is_err());
        assert!(page.count_value().is_err());

        let count = executed_count_handle();
        assert_eq!(count.count_value().unwrap(), 1);
        assert!(count.page_entry_count().is_err());
        assert!(count.exists_value().is_err());

        let exists = executed_exists_handle();
        assert!(exists.exists_value().unwrap());
        assert!(exists.count_value().is_err());
        assert!(exists.row_count().is_err());
    }
}
