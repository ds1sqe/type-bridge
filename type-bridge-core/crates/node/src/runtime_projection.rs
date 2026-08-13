//! Verified package-scoped runtime projections for generated TypeScript models.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use napi::bindgen_prelude::{BigInt, External};
use napi::{Error, Status};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedContainer, ProjectedModelForm, ProjectedMultiplicity, ProjectionConfig,
    RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS, SdkDiagnosticCategory, SdkDiagnosticCode,
    SdkDiagnosticMessage, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
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
    AnswerCancellation, AttributeValue, Database, HydratedAttribute, InstalledRuntimeProjection,
    ProjectedAttributeValue, ProjectedCreate, ProjectedCrudExecutor, ProjectedReference,
    ProjectedRolePlayer, ProjectedThing, ProviderRuntimeOwner, QueryExecutionResourceLimits,
    ThingKind, TransactionContext, ValueType,
};
use type_bridge_schema::{decode_schema_authority, schema_authority_capability_vocabulary};
use type_bridge_schema_codegen::{TypeScriptEmitter, verify_projection_evidence};

use crate::match_runtime::{
    NodeQueryCancellation, NodeQueryExecutionResources, napi_sdk_diagnostic, revalidate_diagnostic,
};
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
    projection: Arc<InstalledRuntimeProjection>,
    types_by_label: BTreeMap<String, TypeId>,
}

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
            Self::Reference(reference) => {
                reference.validate_for(installed)?;
                Ok(reference.as_ref().clone())
            }
        }
    }
}

/// Opaque native proof retained only by an ordered generated facade WeakMap.
pub struct NodeProjectedFacadeProof {
    proof: FacadeProjectionProof,
}

/// One private projected wire and its non-serializable facade proofs.
#[napi]
pub struct NodeProjectedValueEnvelope {
    package: Arc<InstalledPackage>,
    json: String,
    thing: Arc<ProjectedThing>,
}

#[napi]
impl NodeProjectedValueEnvelope {
    /// Return the public generated-value wire without any origin evidence.
    #[napi(getter)]
    pub fn json(&self) -> String {
        self.json.clone()
    }

    /// Clone the opaque proof for the root complete facade.
    #[napi(js_name = "rootProof")]
    pub fn root_proof(&self) -> External<NodeProjectedFacadeProof> {
        External::new(NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Thing(Arc::clone(&self.thing)),
        })
    }

    /// Clone the opaque reference proof for one exact hydrated role facade.
    #[napi(js_name = "roleProof")]
    pub fn role_proof(
        &self,
        role_name: String,
        player_index: u32,
    ) -> napi::Result<External<NodeProjectedFacadeProof>> {
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(self.thing.type_id())
            .ok_or_else(|| runtime_error("projected proof root model is absent"))?;
        let role_id = model
            .query_tokens()
            .roles()
            .iter()
            .find(|(_, role)| role.target_name().as_str() == role_name)
            .map(|(role_id, _)| role_id)
            .ok_or_else(|| runtime_error("projected proof role is absent"))?;
        let player = self
            .thing
            .roles()
            .get(role_id)
            .and_then(|players| players.get(player_index as usize))
            .ok_or_else(|| runtime_error("projected proof role player is absent"))?;
        Ok(External::new(NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Reference(Arc::new(player.reference().clone())),
        }))
    }
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
        schema_authority_json: Option<String>,
    ) -> napi::Result<Self> {
        let runtime = match schema_authority_json {
            Some(schema_authority_json) => install_authority_backed_projection(
                &projection_json,
                &semantic_fingerprint_json,
                &projection_fingerprint_json,
                &schema_authority_json,
            )?,
            None => {
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
                if projection_uses_ordered_collections(&runtime) {
                    return Err(projection_evidence_mismatch());
                }
                verify_legacy_typescript_projection_evidence(&runtime)?;
                runtime
            }
        };
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
        let projection = Arc::new(InstalledRuntimeProjection::try_new(runtime).map_err(orm_error)?);
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
        Ok(NodeMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        ))
    }

    /// Build an opaque match session carrying the common resource policy and
    /// caller-owned cooperative cancellation signal.
    #[napi(js_name = "matchSessionWithResources")]
    pub fn match_session_with_resources(
        &self,
        resources: &NodeQueryExecutionResources,
        cancellation: &NodeQueryCancellation,
    ) -> napi::Result<NodeMatchSessionHandle> {
        let registry = self
            .package
            .projection
            .match_registry()
            .map_err(orm_error)?;
        Ok(NodeMatchSessionHandle::from_installed(
            Arc::clone(&self.package.projection),
            Arc::new(registry),
            resources.inner(),
            cancellation.inner(),
        ))
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
        let expected = projected_value_type(value_type);
        if projection_uses_ordered_collections(self.package.projection.projection()) {
            project_attribute_scalar(&self.package.projection, id, &wire, expected)
                .map(|_| ())
                .map_err(napi_sdk_diagnostic)
        } else {
            let value = scalar_to_attribute(&wire, expected)?;
            self.package
                .projection
                .validate_attribute_value(&id, &value)
                .map_err(|error| invalid_error(error.to_string()))
        }
    }

    /// Validate one generated provider-hydrated attribute scalar through the common Rust contract.
    #[napi(js_name = "validateHydratedAttributeValueJson")]
    pub fn validate_hydrated_attribute_value_json(
        &self,
        type_key: String,
        value_json: String,
    ) -> napi::Result<()> {
        let id = type_id_from_key(&type_key)?;
        let path = [SdkDiagnosticPathSegment::Type(id.clone())];
        if id.kind() != TypeKind::Attribute {
            return Err(napi_sdk_diagnostic(malformed_projected_hydration_at(path)));
        }
        let model = self
            .package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| {
                napi_sdk_diagnostic(generated_token_package_mismatch_at(path.clone()))
            })?;
        let value_type = model
            .declaration()
            .value_type()
            .ok_or_else(|| napi_sdk_diagnostic(malformed_projected_hydration_at(path.clone())))?;
        let wire: ScalarWire = serde_json::from_str(&value_json)
            .map_err(|_| napi_sdk_diagnostic(malformed_projected_hydration_at(path.clone())))?;
        project_hydrated_attribute_scalar(
            &self.package.projection,
            id,
            &wire,
            projected_value_type(value_type),
        )
        .map(|_| ())
        .map_err(napi_sdk_diagnostic)
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
        let expected = projected_value_type(value_type);
        if projection_uses_ordered_collections(self.package.projection.projection()) {
            let projected =
                project_attribute_scalar(&self.package.projection, attribute_id, &wire, expected)
                    .map_err(napi_sdk_diagnostic)?;
            self.package
                .projection
                .validate_canonical_field_value(&id, field.id(), projected.value())
                .map_err(napi_sdk_diagnostic)
        } else {
            let value = scalar_to_attribute(&wire, expected)?;
            self.package
                .projection
                .validate_field_value(&id, &field_name, &value)
                .map_err(|error| invalid_error(error.to_string()))
        }
    }

    /// Validate one complete generated create payload through the common Rust contract.
    #[napi(js_name = "validateCreateJson")]
    pub fn validate_create_json(&self, type_key: String, value_json: String) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), &type_key)?;
        let wire = parse_wire(&value_json)?;
        if wire.type_key != type_key || wire.form != WireForm::Complete {
            return Err(napi_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        if wire.iid.is_some() || wire.value.is_some() {
            return Err(napi_sdk_diagnostic(malformed_projected_create_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_create_wire(self.package.as_ref(), &id, &wire)
            .map(|_| ())
            .map_err(napi_sdk_diagnostic)
    }

    /// Validate one complete generated provider result through the common Rust contract.
    #[napi(js_name = "validateThingJson")]
    pub fn validate_thing_json(&self, type_key: String, value_json: String) -> napi::Result<()> {
        self.validate_thing_json_inner(&type_key, &value_json)
    }

    fn validate_thing_json_inner(&self, type_key: &str, value_json: &str) -> napi::Result<()> {
        let id = manageable_type(self.package.as_ref(), type_key)?;
        let wire: ProjectedWire = serde_json::from_str(value_json).map_err(|_| {
            napi_sdk_diagnostic(malformed_projected_hydration_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ]))
        })?;
        if wire.type_key != type_key || wire.form != WireForm::Complete {
            return Err(napi_sdk_diagnostic(generated_token_package_mismatch_at([
                SdkDiagnosticPathSegment::Type(id.clone()),
            ])));
        }
        project_thing_wire(self.package.as_ref(), &id, &wire)
            .map(|_| ())
            .map_err(napi_sdk_diagnostic)
    }

    /// Surface a stable package-boundary diagnostic at an exact create member path.
    #[napi(js_name = "rejectGeneratedTokenPackageMismatch")]
    pub fn reject_generated_token_package_mismatch(&self, path_json: String) -> napi::Result<()> {
        let path: Vec<GeneratedPackagePathWire> = serde_json::from_str(&path_json)
            .map_err(|error| invalid_error(format!("invalid generated package path: {error}")))?;
        let path = generated_package_path(self.package.as_ref(), &path)?;
        Err(napi_sdk_diagnostic(generated_token_package_mismatch_at(
            path,
        )))
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

    /// Materialize one successor query thing with its exact opaque proof.
    #[napi(js_name = "materializeMatchThingProjected")]
    pub fn materialize_match_thing_projected(
        &self,
        thing: &NodeValidatedThingHandle,
    ) -> napi::Result<NodeProjectedValueEnvelope> {
        if !projection_uses_ordered_collections(self.package.projection.projection()) {
            return Err(invalid_error(
                "projected query proof materialization is reserved for the ordered successor runtime",
            ));
        }
        let projected = thing.projected_thing().ok_or_else(|| {
            runtime_error("validated query thing has no projected successor companion")
        })?;
        projected_envelope_arc(Arc::clone(&self.package), projected)
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
    /// Insert one ordered successor value through the common projected executor.
    #[napi(js_name = "insertProjected")]
    pub fn insert_projected(
        &self,
        instance_json: String,
        proofs: Vec<Option<&External<NodeProjectedFacadeProof>>>,
    ) -> napi::Result<NodeProjectedValueEnvelope> {
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let projected = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => self
                .runtime
                .block_on(executor.insert_entity(database, &instance)),
            (TypeKind::Entity, None, Some(transaction)) => self
                .runtime
                .block_on(executor.insert_entity_in_transaction(transaction, &instance)),
            (TypeKind::Relation, Some(database), None) => self
                .runtime
                .block_on(executor.insert_relation(database, &instance)),
            (TypeKind::Relation, None, Some(transaction)) => self
                .runtime
                .block_on(executor.insert_relation_in_transaction(transaction, &instance)),
            _ => return Err(runtime_error("projected manager has no execution target")),
        }
        .map_err(napi_sdk_diagnostic)?;
        projected_envelope(Arc::clone(&self.package), projected)
    }

    /// Put one ordered successor value through the common projected executor.
    #[napi(js_name = "putProjected")]
    pub fn put_projected(
        &self,
        instance_json: String,
        proofs: Vec<Option<&External<NodeProjectedFacadeProof>>>,
    ) -> napi::Result<NodeProjectedValueEnvelope> {
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let projected = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => self
                .runtime
                .block_on(executor.put_entity(database, &instance)),
            (TypeKind::Entity, None, Some(transaction)) => self
                .runtime
                .block_on(executor.put_entity_in_transaction(transaction, &instance)),
            (TypeKind::Relation, Some(database), None) => self
                .runtime
                .block_on(executor.put_relation(database, &instance)),
            (TypeKind::Relation, None, Some(transaction)) => self
                .runtime
                .block_on(executor.put_relation_in_transaction(transaction, &instance)),
            _ => return Err(runtime_error("projected manager has no execution target")),
        }
        .map_err(napi_sdk_diagnostic)?;
        projected_envelope(Arc::clone(&self.package), projected)
    }

    /// Replace one ordered successor value through the common projected executor.
    #[napi(js_name = "updateProjected")]
    pub fn update_projected(
        &self,
        iid: String,
        instance_json: String,
        proofs: Vec<Option<&External<NodeProjectedFacadeProof>>>,
    ) -> napi::Result<NodeProjectedValueEnvelope> {
        let instance = self.projected_create_input(&instance_json, &proofs)?;
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let projected = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => self
                .runtime
                .block_on(executor.update_entity(database, &iid, &instance)),
            (TypeKind::Entity, None, Some(transaction)) => self
                .runtime
                .block_on(executor.update_entity_in_transaction(transaction, &iid, &instance)),
            (TypeKind::Relation, Some(database), None) => self
                .runtime
                .block_on(executor.update_relation(database, &iid, &instance)),
            (TypeKind::Relation, None, Some(transaction)) => self
                .runtime
                .block_on(executor.update_relation_in_transaction(transaction, &iid, &instance)),
            _ => return Err(runtime_error("projected manager has no execution target")),
        }
        .map_err(napi_sdk_diagnostic)?;
        projected_envelope(Arc::clone(&self.package), projected)
    }

    /// Read one ordered successor value through the common projected executor.
    #[napi(js_name = "getByIidProjected")]
    pub fn get_by_iid_projected(
        &self,
        iid: String,
    ) -> napi::Result<Option<NodeProjectedValueEnvelope>> {
        if !projection_uses_ordered_collections(self.package.projection.projection()) {
            return Err(invalid_error(
                "common projected execution is reserved for the ordered successor runtime",
            ));
        }
        let executor = ProjectedCrudExecutor::new(self.package.projection.as_ref());
        let projected = match (self.type_id.kind(), &self.database, &self.transaction) {
            (TypeKind::Entity, Some(database), None) => self
                .runtime
                .block_on(executor.get_entity_by_iid(database, &self.type_id, &iid)),
            (TypeKind::Entity, None, Some(transaction)) => {
                self.runtime
                    .block_on(executor.get_entity_by_iid_in_transaction(
                        transaction,
                        &self.type_id,
                        &iid,
                    ))
            }
            (TypeKind::Relation, Some(database), None) => self
                .runtime
                .block_on(executor.get_relation_by_iid(database, &self.type_id, &iid)),
            (TypeKind::Relation, None, Some(transaction)) => {
                self.runtime
                    .block_on(executor.get_relation_by_iid_in_transaction(
                        transaction,
                        &self.type_id,
                        &iid,
                    ))
            }
            _ => return Err(runtime_error("projected manager has no execution target")),
        }
        .map_err(napi_sdk_diagnostic)?;
        projected
            .map(|projected| projected_envelope(Arc::clone(&self.package), projected))
            .transpose()
    }

    /// Insert one exact complete value and return its hydrated private wire.
    #[napi(js_name = "insertJson")]
    pub fn insert_json(&self, instance_json: String) -> napi::Result<String> {
        let mut instance = parse_wire(&instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        self.validate_ordered_single_write(&instance)?;
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
        self.validate_ordered_single_write(&instance)?;
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
        self.validate_ordered_single_write(&instance)?;
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
    fn projected_create_input(
        &self,
        instance_json: &str,
        proofs: &[Option<&External<NodeProjectedFacadeProof>>],
    ) -> napi::Result<ProjectedCreate> {
        if !projection_uses_ordered_collections(self.package.projection.projection()) {
            return Err(invalid_error(
                "common projected execution is reserved for the ordered successor runtime",
            ));
        }
        let instance = parse_wire(instance_json)?;
        ensure_root_wire(self.package.as_ref(), &instance, &self.type_id)?;
        let proofs = proofs
            .iter()
            .map(|proof| proof.map(AsRef::as_ref))
            .collect::<Vec<_>>();
        project_create_wire_with_proofs(self.package.as_ref(), &self.type_id, &instance, &proofs)
            .map_err(napi_sdk_diagnostic)
    }

    fn validate_ordered_single_write(&self, instance: &ProjectedWire) -> napi::Result<()> {
        if projection_uses_ordered_collections(self.package.projection.projection()) {
            project_create_wire(self.package.as_ref(), &self.type_id, instance)
                .map(|_| ())
                .map_err(napi_sdk_diagnostic)?;
        }
        Ok(())
    }

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

fn projected_envelope(
    package: Arc<InstalledPackage>,
    thing: ProjectedThing,
) -> napi::Result<NodeProjectedValueEnvelope> {
    projected_envelope_arc(package, Arc::new(thing))
}

fn projected_envelope_arc(
    package: Arc<InstalledPackage>,
    thing: Arc<ProjectedThing>,
) -> napi::Result<NodeProjectedValueEnvelope> {
    let json = wire_json(&projected_thing_wire(package.as_ref(), thing.as_ref())?)?;
    Ok(NodeProjectedValueEnvelope {
        package,
        json,
        thing,
    })
}

fn install_authority_backed_projection(
    projection_json: &str,
    semantic_fingerprint_json: &str,
    projection_fingerprint_json: &str,
    schema_authority_json: &str,
) -> napi::Result<RuntimeProjection> {
    let runtime = decode_runtime_projection_verified(
        projection_json.as_bytes(),
        semantic_fingerprint_json.as_bytes(),
        projection_fingerprint_json.as_bytes(),
    )
    .map_err(|_| projection_evidence_mismatch())?;
    if runtime.target() != BindingTarget::TypeScript {
        return Err(projection_evidence_mismatch());
    }
    let authority = decode_schema_authority(
        schema_authority_json.as_bytes(),
        &schema_authority_capability_vocabulary(),
    )
    .map_err(|_| projection_evidence_mismatch())?;
    verify_projection_evidence(&authority, &runtime).map_err(|_| projection_evidence_mismatch())?;
    Ok(runtime)
}

fn projection_evidence_mismatch() -> Error {
    napi_sdk_diagnostic(SdkExecutionDiagnostic::projection_evidence_mismatch())
}

fn verify_legacy_typescript_projection_evidence(runtime: &RuntimeProjection) -> napi::Result<()> {
    if projection_uses_ordered_collections(runtime) {
        return Err(invalid_error(
            "ordered TypeScript runtime projections require compiled schema authority",
        ));
    }
    let emitter = TypeScriptEmitter::new();
    let resources = emitter.code_resources().map_err(diagnostic_error)?;
    if runtime.config() != &ProjectionConfig::typescript()
        || runtime.generator_handlers() != emitter.generator_handlers()
        || runtime.code_resources() != resources
    {
        return Err(invalid_error(
            "legacy TypeScript runtime projection does not match the exact shipped handler and resource evidence",
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScalarWire {
    value_type: ValueTypeTag,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum GeneratedPackagePathWire {
    Type {
        #[serde(rename = "typeKey")]
        type_key: String,
    },
    Field {
        name: String,
    },
    Role {
        name: String,
    },
    Index {
        value: u32,
    },
}

fn generated_package_path(
    package: &InstalledPackage,
    path: &[GeneratedPackagePathWire],
) -> napi::Result<Vec<SdkDiagnosticPathSegment>> {
    if path.is_empty()
        || path.len() > MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS
        || !path.len().is_multiple_of(3)
    {
        return Err(invalid_error(
            "generated package mismatch path has an invalid shape",
        ));
    }
    let mut lowered = Vec::with_capacity(path.len());
    for chunk in path.chunks_exact(3) {
        let GeneratedPackagePathWire::Type { type_key } = &chunk[0] else {
            return Err(invalid_error(
                "generated package mismatch path requires a projected type",
            ));
        };
        let id = manageable_type(package, type_key)?;
        let model = package
            .projection
            .projection()
            .models()
            .get(&id)
            .ok_or_else(|| invalid_error("projection model is absent"))?;
        let member = match &chunk[1] {
            GeneratedPackagePathWire::Field { name } => model
                .query_tokens()
                .fields()
                .values()
                .find(|field| field.target_name().as_str() == name)
                .map(|field| SdkDiagnosticPathSegment::Field(field.id().clone())),
            GeneratedPackagePathWire::Role { name } => model
                .query_tokens()
                .roles()
                .iter()
                .find(|(_, role)| role.target_name().as_str() == name)
                .map(|(role, _)| SdkDiagnosticPathSegment::Role(role.clone())),
            _ => None,
        }
        .ok_or_else(|| invalid_error("generated package mismatch member is not projected"))?;
        let GeneratedPackagePathWire::Index { value } = &chunk[2] else {
            return Err(invalid_error(
                "generated package mismatch member requires an index",
            ));
        };
        lowered.extend([
            SdkDiagnosticPathSegment::Type(id),
            member,
            SdkDiagnosticPathSegment::Index(u64::from(*value)),
        ]);
    }
    Ok(lowered)
}

fn project_create_wire(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> Result<ProjectedCreate, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            malformed_projected_create_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    let mut allowed = BTreeSet::new();
    let mut fields = Vec::with_capacity(model.create().fields().len());
    for field in model.create().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Field(field.token().clone()),
            ],
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_attribute_wire(package, value).map_err(|diagnostic| {
                    rebase_sdk_diagnostic(
                        diagnostic,
                        [
                            projected_path(),
                            SdkDiagnosticPathSegment::Field(field.token().clone()),
                            SdkDiagnosticPathSegment::Index(projected_index(index)),
                        ],
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::with_capacity(model.create().roles().len());
    for (role_id, role) in model.create().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
            ],
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_reference_wire(
                    package,
                    role.players(),
                    value,
                    &[
                        projected_path(),
                        SdkDiagnosticPathSegment::Role(role_id.clone()),
                        SdkDiagnosticPathSegment::Index(projected_index(index)),
                    ],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        roles.push((role_id.clone(), projected));
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_create_at([projected_path()]));
    }
    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles)
}

fn project_create_wire_with_proofs(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
    proofs: &[Option<&NodeProjectedFacadeProof>],
) -> Result<ProjectedCreate, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            malformed_projected_create_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    let mut allowed = BTreeSet::new();
    let mut fields = Vec::with_capacity(model.create().fields().len());
    for field in model.create().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Field(field.token().clone()),
            ],
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                project_attribute_wire(package, value).map_err(|diagnostic| {
                    rebase_sdk_diagnostic(
                        diagnostic,
                        [
                            projected_path(),
                            SdkDiagnosticPathSegment::Field(field.token().clone()),
                            SdkDiagnosticPathSegment::Index(projected_index(index)),
                        ],
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut proof_index = 0_usize;
    let mut roles = Vec::with_capacity(model.create().roles().len());
    for (role_id, role) in model.create().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_create_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
            ],
        )?;
        let mut projected = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let operation_path = [
                projected_path(),
                SdkDiagnosticPathSegment::Role(role_id.clone()),
                SdkDiagnosticPathSegment::Index(projected_index(index)),
            ];
            let proof = proofs.get(proof_index).copied().flatten();
            proof_index = proof_index.saturating_add(1);
            projected.push(project_reference_wire_with_proof(
                package,
                role.players(),
                value,
                &operation_path,
                proof,
            )?);
        }
        roles.push((role_id.clone(), projected));
    }
    if proof_index != proofs.len()
        || wire
            .values
            .keys()
            .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_create_at([projected_path()]));
    }
    ProjectedCreate::try_new(&package.projection, id.clone(), fields, roles)
}

fn project_thing_wire(
    package: &InstalledPackage,
    id: &TypeId,
    wire: &ProjectedWire,
) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| {
            generated_token_package_mismatch_at([SdkDiagnosticPathSegment::Type(id.clone())])
        })?;
    let projected_path = || SdkDiagnosticPathSegment::Type(id.clone());
    if wire.value.is_some() {
        return Err(malformed_projected_hydration_at([projected_path()]));
    }

    let mut allowed = BTreeSet::new();
    let mut fields = Vec::<(OwnsFactId, Vec<ProjectedAttributeValue>)>::with_capacity(
        model.complete_read().fields().len(),
    );
    for field in model.complete_read().fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field.token())
            .ok_or_else(|| malformed_projected_hydration_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let path = [
            projected_path(),
            SdkDiagnosticPathSegment::Field(field.token().clone()),
        ];
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            field.multiplicity(),
            path.iter().cloned(),
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut value_path = path.to_vec();
                value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
                project_hydrated_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.push((field.token().clone(), projected));
    }

    let mut roles = Vec::<(RoleId, Vec<ProjectedRolePlayer>)>::with_capacity(
        model.complete_read().roles().len(),
    );
    for (role_id, role) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| malformed_projected_hydration_at([projected_path()]))?;
        allowed.insert(token.target_name().as_str());
        let path = [
            projected_path(),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
        ];
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            role.multiplicity(),
            path.iter().cloned(),
        )?;
        let projected = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut value_path = path.to_vec();
                value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
                let reference =
                    project_hydrated_reference_wire(package, role.players(), value, &value_path)?;
                ProjectedRolePlayer::try_new(&package.projection, reference)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        roles.push((role_id.clone(), projected));
    }
    if wire
        .values
        .keys()
        .any(|name| !allowed.contains(name.as_str()))
    {
        return Err(malformed_projected_hydration_at([projected_path()]));
    }

    ProjectedThing::try_new(
        &package.projection,
        id.clone(),
        wire.iid.clone().unwrap_or_default(),
        fields,
        roles,
    )
}

fn projected_hydration_wire_items(
    value: Option<&Value>,
    multiplicity: ProjectedMultiplicity,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> Result<Vec<ProjectedWire>, SdkExecutionDiagnostic> {
    let path = path.into_iter().collect::<Vec<_>>();
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if multiplicity.container() == ProjectedContainer::Sequence => {
            values
                .iter()
                .map(|value| {
                    serde_json::from_value(value.clone())
                        .map_err(|_| malformed_projected_hydration_at(path.clone()))
                })
                .collect()
        }
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            serde_json::from_value(value.clone())
                .map(|value| vec![value])
                .map_err(|_| malformed_projected_hydration_at(path))
        }
        Some(_) => Err(malformed_projected_hydration_at(path)),
    }
}

fn project_hydrated_attribute_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_hydration_at(std::iter::empty()))?;
    let path = [SdkDiagnosticPathSegment::Type(id.clone())];
    if id.kind() != TypeKind::Attribute
        || wire.form != WireForm::Complete
        || wire.iid.is_some()
        || !wire.values.is_empty()
    {
        return Err(malformed_projected_hydration_at(path));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(path.clone()))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| malformed_projected_hydration_at(path.clone()))?;
    let scalar = wire
        .value
        .as_ref()
        .ok_or_else(|| malformed_projected_hydration_at(path.clone()))?;
    project_hydrated_attribute_scalar(
        &package.projection,
        id,
        scalar,
        projected_value_type(value_type),
    )
}

fn project_hydrated_attribute_scalar(
    projection: &InstalledRuntimeProjection,
    id: TypeId,
    wire: &ScalarWire,
    expected: ValueType,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let value = scalar_to_ordered_attribute(wire, expected).map_err(|_| {
        wrong_hydrated_scalar_domain_at([SdkDiagnosticPathSegment::Type(id.clone())])
    })?;
    ProjectedAttributeValue::try_from_hydrated_attribute_value(projection, id, &value)
}

fn project_hydrated_reference_wire(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
    let form = match wire.form {
        WireForm::Complete => ProjectedModelForm::Complete,
        WireForm::Reference => ProjectedModelForm::Reference,
    };
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) || wire.value.is_some() {
        return Err(malformed_projected_hydration_at(
            operation_path.iter().cloned(),
        ));
    }
    if !allowed_players
        .iter()
        .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(malformed_projected_hydration_at(
            operation_path.iter().cloned(),
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(operation_path.iter().cloned()))?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| malformed_projected_hydration_at(operation_path.iter().cloned()))?;
        let mut key_path = operation_path.to_vec();
        key_path.extend([
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Field(key_id.clone()),
        ]);
        let values = projected_hydration_wire_items(
            wire.values.get(token.target_name().as_str()),
            read.multiplicity(),
            key_path.iter().cloned(),
        )?;
        for (index, value) in values.iter().enumerate() {
            let mut value_path = key_path.clone();
            value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
            keys.push((
                key_id.clone(),
                project_hydrated_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_hydration_diagnostic(diagnostic, value_path))?,
            ));
        }
    }
    ProjectedReference::try_new_for_hydration(&package.projection, id, wire.iid.clone(), keys)
        .map_err(|diagnostic| {
            rebase_hydration_diagnostic(diagnostic, operation_path.iter().cloned())
        })
}

fn projected_wire_items(
    value: Option<&Value>,
    multiplicity: ProjectedMultiplicity,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> Result<Vec<ProjectedWire>, SdkExecutionDiagnostic> {
    let path = path.into_iter().collect::<Vec<_>>();
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if multiplicity.container() == ProjectedContainer::Sequence => {
            values
                .iter()
                .map(|value| {
                    serde_json::from_value(value.clone())
                        .map_err(|_| malformed_projected_create_at(path.clone()))
                })
                .collect()
        }
        Some(value) if multiplicity.container() == ProjectedContainer::Scalar => {
            serde_json::from_value(value.clone())
                .map(|value| vec![value])
                .map_err(|_| malformed_projected_create_at(path))
        }
        Some(_) => Err(malformed_projected_create_at(path)),
    }
}

fn project_attribute_wire(
    package: &InstalledPackage,
    wire: &ProjectedWire,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_create_at(std::iter::empty()))?;
    let path = [SdkDiagnosticPathSegment::Type(id.clone())];
    if id.kind() != TypeKind::Attribute
        || wire.form != WireForm::Complete
        || !wire.values.is_empty()
    {
        return Err(malformed_projected_create_at(path));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| generated_token_package_mismatch_at(path.clone()))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| malformed_projected_create_at(path.clone()))?;
    let scalar = wire
        .value
        .as_ref()
        .ok_or_else(|| malformed_projected_create_at(path))?;
    project_attribute_scalar(
        &package.projection,
        id,
        scalar,
        projected_value_type(value_type),
    )
}

fn project_attribute_scalar(
    projection: &InstalledRuntimeProjection,
    id: TypeId,
    wire: &ScalarWire,
    expected: ValueType,
) -> Result<ProjectedAttributeValue, SdkExecutionDiagnostic> {
    let value = scalar_to_ordered_attribute(wire, expected)
        .map_err(|_| wrong_scalar_domain_at([SdkDiagnosticPathSegment::Type(id.clone())]))?;
    ProjectedAttributeValue::try_from_attribute_value(projection, id, &value)
}

fn project_reference_wire(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let id = type_id_from_key(&wire.type_key)
        .map_err(|_| malformed_projected_create_at(operation_path.iter().cloned()))?;
    let form = match wire.form {
        WireForm::Complete => ProjectedModelForm::Complete,
        WireForm::Reference => ProjectedModelForm::Reference,
    };
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation)
        || wire.value.is_some()
        || !allowed_players
            .iter()
            .any(|player| player.id() == &id && player.form() == form)
    {
        return Err(malformed_projected_create_at(
            operation_path.iter().cloned(),
        ));
    }
    let model = package
        .projection
        .projection()
        .models()
        .get(&id)
        .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
    let mut keys = Vec::new();
    for key_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| malformed_projected_create_at(operation_path.iter().cloned()))?;
        let mut key_path = operation_path.to_vec();
        key_path.extend([
            SdkDiagnosticPathSegment::Type(id.clone()),
            SdkDiagnosticPathSegment::Field(key_id.clone()),
        ]);
        let values = projected_wire_items(
            wire.values.get(token.target_name().as_str()),
            read.multiplicity(),
            key_path.iter().cloned(),
        )?;
        for (index, value) in values.iter().enumerate() {
            let mut value_path = key_path.clone();
            value_path.push(SdkDiagnosticPathSegment::Index(projected_index(index)));
            keys.push((
                key_id.clone(),
                project_attribute_wire(package, value)
                    .map_err(|diagnostic| rebase_sdk_diagnostic(diagnostic, value_path))?,
            ));
        }
    }
    ProjectedReference::try_new(&package.projection, id, wire.iid.clone(), keys).map_err(
        |diagnostic| {
            let mut path = operation_path.to_vec();
            path.extend(diagnostic.path().iter().cloned());
            rebase_sdk_diagnostic(diagnostic, path)
        },
    )
}

fn project_reference_wire_with_proof(
    package: &InstalledPackage,
    allowed_players: &BTreeSet<type_bridge_contract::projection::ProjectedModelUse>,
    wire: &ProjectedWire,
    operation_path: &[SdkDiagnosticPathSegment],
    proof: Option<&NodeProjectedFacadeProof>,
) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
    let visible = project_reference_wire(package, allowed_players, wire, operation_path)?;
    let Some(proof) = proof else {
        return Ok(visible);
    };
    let retained = proof
        .proof
        .reference(&package.projection)
        .map_err(|diagnostic| rebase_sdk_diagnostic(diagnostic, operation_path.iter().cloned()))?;
    if visible != retained {
        return Err(malformed_projected_create_at(
            operation_path.iter().cloned(),
        ));
    }
    Ok(retained)
}

fn rebase_sdk_diagnostic(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    let rebased = match diagnostic.category() {
        SdkDiagnosticCategory::InvalidInput => {
            SdkExecutionDiagnostic::invalid_input(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::ResourceLimit => {
            SdkExecutionDiagnostic::resource_limit(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::Integrity => {
            SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message())
        }
        _ => return malformed_projected_create_at(path),
    };
    let rebased = append_sdk_path(rebased, path);
    diagnostic
        .details()
        .iter()
        .fold(rebased, |rebased, (name, detail)| {
            rebased
                .try_with_detail(name.clone(), detail.clone())
                .expect("common projected-reference details fit the SDK diagnostic contract")
        })
}

fn rebase_hydration_diagnostic(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    let rebased = match diagnostic.category() {
        SdkDiagnosticCategory::ResourceLimit => {
            SdkExecutionDiagnostic::resource_limit(diagnostic.code().clone(), diagnostic.message())
        }
        SdkDiagnosticCategory::Integrity => {
            SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message())
        }
        _ => return malformed_projected_hydration_at(path),
    };
    let rebased = append_sdk_path(rebased, path);
    diagnostic
        .details()
        .iter()
        .fold(rebased, |rebased, (name, detail)| {
            rebased
                .try_with_detail(name.clone(), detail.clone())
                .expect("common projected-hydration details fit the SDK diagnostic contract")
        })
}

fn wrong_scalar_domain_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ),
        path,
    )
}

fn malformed_projected_create_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::invalid_input(
            SdkDiagnosticCode::new("malformed_projected_create")
                .expect("static generated-create code is canonical"),
            SdkDiagnosticMessage::new("generated create payload has an invalid projected shape")
                .expect("static generated-create message is canonical"),
        ),
        path,
    )
}

fn wrong_hydrated_scalar_domain_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("wrong_scalar_domain")
                .expect("static projected-value code is canonical"),
            SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                .expect("static projected-value message is canonical"),
        ),
        path,
    )
}

fn malformed_projected_hydration_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::integrity(
            SdkDiagnosticCode::new("malformed_projected_hydration")
                .expect("static generated-hydration code is canonical"),
            SdkDiagnosticMessage::new("generated hydration payload has an invalid projected shape")
                .expect("static generated-hydration message is canonical"),
        ),
        path,
    )
}

fn generated_token_package_mismatch_at(
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_sdk_path(
        SdkExecutionDiagnostic::generated_token_package_mismatch(),
        path,
    )
}

fn append_sdk_path(
    diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    path.into_iter().fold(diagnostic, |diagnostic, segment| {
        diagnostic
            .try_at(segment)
            .expect("a generated create operation path fits the SDK diagnostic contract")
    })
}

fn projected_index(index: usize) -> u64 {
    u64::try_from(index).expect("a projected collection index fits the SDK diagnostic contract")
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

fn projected_thing_wire(
    package: &InstalledPackage,
    thing: &ProjectedThing,
) -> napi::Result<ProjectedWire> {
    thing
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let id = thing.type_id();
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("projected thing model is absent"))?;
    let mut values = projected_fields_wire(package, id, thing.fields(), false)?;
    if thing.roles().len() != model.complete_read().roles().len() {
        return Err(runtime_error(
            "projected thing roles do not match its complete-read projection",
        ));
    }
    for (role_id, read) in model.complete_read().roles() {
        let token = model
            .query_tokens()
            .roles()
            .get(role_id)
            .ok_or_else(|| runtime_error("projected read role has no query token"))?;
        let players = thing
            .roles()
            .get(role_id)
            .ok_or_else(|| runtime_error("projected thing omitted a selected role"))?
            .iter()
            .map(|player| projected_role_player_wire(package, player))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(players, read.multiplicity())?,
        );
    }
    Ok(ProjectedWire {
        type_key: canonical_type_key(id)?,
        form: WireForm::Complete,
        iid: Some(thing.iid().to_owned()),
        value: None,
        values,
    })
}

fn projected_role_player_wire(
    package: &InstalledPackage,
    player: &ProjectedRolePlayer,
) -> napi::Result<ProjectedWire> {
    let form = match player.exact_form() {
        Some(ProjectedModelForm::Complete) => WireForm::Complete,
        Some(ProjectedModelForm::Reference) => WireForm::Reference,
        None => {
            return Err(runtime_error(
                "successor hydration requires exact role-player form evidence",
            ));
        }
    };
    let values = match form {
        WireForm::Complete => {
            projected_fields_wire(package, player.type_id(), player.fields(), false)?
        }
        WireForm::Reference => projected_reference_fields_wire(package, player.reference())?,
    };
    Ok(ProjectedWire {
        type_key: canonical_type_key(player.type_id())?,
        form,
        iid: Some(player.iid().to_owned()),
        value: None,
        values,
    })
}

fn projected_reference_fields_wire(
    package: &InstalledPackage,
    reference: &ProjectedReference,
) -> napi::Result<BTreeMap<String, Value>> {
    reference
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let model = package
        .projection
        .projection()
        .models()
        .get(reference.type_id())
        .ok_or_else(|| runtime_error("projected reference model is absent"))?;
    if reference.keys().len() != model.reference_read().key_fields().len() {
        return Err(runtime_error(
            "projected hydrated reference keys do not match its reference projection",
        ));
    }
    let mut values = BTreeMap::new();
    for field_id in model.reference_read().key_fields() {
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected reference key has no query token"))?;
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == field_id)
            .ok_or_else(|| runtime_error("projected reference key has no read projection"))?;
        let value = reference
            .keys()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated reference omitted a key"))?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(
                vec![projected_attribute_value_wire(package, value)?],
                read.multiplicity(),
            )?,
        );
    }
    Ok(values)
}

fn projected_fields_wire(
    package: &InstalledPackage,
    id: &TypeId,
    fields: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    reference: bool,
) -> napi::Result<BTreeMap<String, Value>> {
    let model = package
        .projection
        .projection()
        .models()
        .get(id)
        .ok_or_else(|| runtime_error("projected hydrated model is absent"))?;
    let selected = model
        .complete_read()
        .fields()
        .iter()
        .filter(|field| !reference || model.reference_read().key_fields().contains(field.token()))
        .collect::<Vec<_>>();
    if fields.len() != selected.len() {
        return Err(runtime_error(
            "projected hydrated fields do not match the selected model form",
        ));
    }
    let mut values = BTreeMap::new();
    for read in selected {
        let field_id = read.token();
        let token = model
            .query_tokens()
            .fields()
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated field has no query token"))?;
        let projected = fields
            .get(field_id)
            .ok_or_else(|| runtime_error("projected hydrated model omitted a selected field"))?;
        let hydrated = projected
            .iter()
            .map(|value| projected_attribute_value_wire(package, value))
            .collect::<napi::Result<Vec<_>>>()?;
        values.insert(
            token.target_name().as_str().to_owned(),
            projected_read_member_value(hydrated, read.multiplicity())?,
        );
    }
    Ok(values)
}

fn projected_attribute_value_wire(
    package: &InstalledPackage,
    value: &ProjectedAttributeValue,
) -> napi::Result<ProjectedWire> {
    value
        .validate_for(&package.projection)
        .map_err(napi_sdk_diagnostic)?;
    let model = package
        .projection
        .projection()
        .models()
        .get(value.attribute_type())
        .ok_or_else(|| runtime_error("projected attribute model is absent"))?;
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| runtime_error("projected attribute has no scalar domain"))?;
    Ok(ProjectedWire {
        type_key: canonical_type_key(value.attribute_type())?,
        form: WireForm::Complete,
        iid: None,
        value: Some(attribute_to_scalar(
            &value.to_attribute_value(),
            projected_value_type(value_type),
        )?),
        values: BTreeMap::new(),
    })
}

fn projected_read_member_value(
    values: Vec<ProjectedWire>,
    multiplicity: ProjectedMultiplicity,
) -> napi::Result<Value> {
    let count = u64::try_from(values.len())
        .map_err(|_| runtime_error("projected hydrated value count exceeds u64"))?;
    let cardinality = multiplicity.cardinality();
    if count < cardinality.min() || cardinality.max().is_some_and(|maximum| count > maximum) {
        return Err(runtime_error(
            "projected hydrated value violates its projected cardinality",
        ));
    }
    match multiplicity.container() {
        ProjectedContainer::Scalar => values
            .into_iter()
            .next()
            .map(serde_json::to_value)
            .transpose()
            .map_err(json_error)
            .map(|value| value.unwrap_or(Value::Null)),
        ProjectedContainer::Sequence => serde_json::to_value(values).map_err(json_error),
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

fn scalar_to_ordered_attribute(
    wire: &ScalarWire,
    expected: ValueType,
) -> napi::Result<AttributeValue> {
    let timezone = match expected {
        ValueType::DateTime => false,
        ValueType::DateTimeTz => true,
        _ => return scalar_to_attribute(wire, expected),
    };
    let value = wire
        .value
        .as_str()
        .map(|value| Value::String(canonical_typescript_datetime(value, timezone)))
        .unwrap_or_else(|| wire.value.clone());
    scalar_to_attribute(
        &ScalarWire {
            value_type: wire.value_type,
            value,
        },
        expected,
    )
}

fn canonical_typescript_datetime(value: &str, timezone: bool) -> String {
    let local = if timezone {
        let Some(local) = value.strip_suffix('Z') else {
            return value.to_owned();
        };
        local
    } else {
        value
    };
    let Some((whole, fraction)) = local.rsplit_once('.') else {
        return value.to_owned();
    };
    if fraction.len() != 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_owned();
    }
    let fraction = fraction.trim_end_matches('0');
    let local = if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    };
    if timezone { format!("{local}Z") } else { local }
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
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{
        CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_schema::{
        ManagedDeltaContext, SchemaDocumentSet, VerifiedSchemaAuthority, build_schema_authority,
        encode_schema_authority, normalize_documents, project,
    };
    use type_bridge_schema_codegen::{PythonEmitter, TypeScriptEmitter};

    use super::*;

    const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag:
    value:
      type: string
      regex: "^[a-z]+$"
  score: { value: integer }
  val-double: { value: double }
  val-datetime: { value: datetime }
  val-datetime-tz: { value: datetime-tz }
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
    owns:
      identifier: { key: true }
      score: { card: 1, range: { min: 1, max: 5 } }
  group:
    sub: actor
relations:
  membership:
    relates:
      member:
        card: { min: 1, max: 4 }
        ordered: true
        distinct: true
  base-activity:
    relates:
      participant: { abstract: true, card: 1 }
  plain-activity:
    sub: base-activity
  activity-link:
    relates:
      subject:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
plays:
  person:
    membership:
      member: { card: { min: 0, max: 4 } }
    base-activity:
      participant: { card: { min: 0, max: 1 } }
  group:
    base-activity:
      participant: { card: { min: 0, max: 1 } }
  plain-activity:
    activity-link:
      subject: { card: { min: 0, max: 4 } }
"#;

    const UNORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  score: { value: integer }
entities:
  person:
    owns:
      score: { card: 1, range: { min: 1, max: 5 } }
"#;

    fn authority(source: &str, document: &str) -> VerifiedSchemaAuthority {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new(document).unwrap(), source)]).unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("node-native").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            schema_authority_capability_vocabulary(),
        );
        build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap()
    }

    fn typescript_projection(authority: &VerifiedSchemaAuthority) -> RuntimeProjection {
        let emitter = TypeScriptEmitter::new();
        project(
            authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap()
    }

    fn evidence(projection: &RuntimeProjection) -> (String, String, String) {
        (
            String::from_utf8(to_canonical_json(projection).unwrap()).unwrap(),
            String::from_utf8(to_canonical_json(projection.semantic_fingerprint()).unwrap())
                .unwrap(),
            String::from_utf8(to_canonical_json(projection.projection_fingerprint()).unwrap())
                .unwrap(),
        )
    }

    fn registrations(projection: &RuntimeProjection) -> String {
        serde_json::to_string(
            &projection
                .models()
                .iter()
                .map(|(id, model)| {
                    serde_json::json!({
                        "typeKey": canonical_type_key(id).unwrap(),
                        "targetName": model.target_name().as_str(),
                        "create": model.create().enabled(),
                        "reference": model.reference_read().target_name().is_some(),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn assert_projection_evidence_mismatch(error: Error) {
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(
            diagnostic,
            serde_json::json!({
                "category": "integrity",
                "sdkCategory": "integrity",
                "queryCategory": null,
                "code": "projection_evidence_mismatch",
                "message": "Generated projection evidence does not match the verified schema package",
                "path": [{"kind": "argument", "value": "projection_evidence"}],
                "details": {},
            })
        );
    }

    fn runtime_for_schema(source: &str, document: &str) -> NodeRuntimeProjection {
        let authority = authority(source, document);
        let authority_json = String::from_utf8(encode_schema_authority(&authority)).unwrap();
        let projection = typescript_projection(&authority);
        let registrations = registrations(&projection);
        let (projection, semantic, fingerprint) = evidence(&projection);
        NodeRuntimeProjection::new(
            projection,
            semantic,
            fingerprint,
            registrations,
            Some(authority_json),
        )
        .unwrap()
    }

    fn ordered_runtime() -> NodeRuntimeProjection {
        runtime_for_schema(ORDERED_SCHEMA, "node-create-admission.yaml")
    }

    fn manager_without_execution_target(
        runtime: &NodeRuntimeProjection,
        type_id: TypeId,
    ) -> NodeProjectedModelManager {
        NodeProjectedModelManager {
            package: Arc::clone(&runtime.package),
            type_id,
            database: None,
            transaction: None,
            runtime: Arc::new(
                ProviderRuntimeOwner::new().expect("provider runtime should start for native test"),
            ),
            filters: vec![],
        }
    }

    fn type_key(kind: TypeKind, label: &str) -> String {
        canonical_type_key(&TypeId::new(kind, label).unwrap()).unwrap()
    }

    fn attribute_wire(label: &str, value_type: ValueTypeTag, value: Value) -> ProjectedWire {
        ProjectedWire {
            type_key: type_key(TypeKind::Attribute, label),
            form: WireForm::Complete,
            iid: None,
            value: Some(ScalarWire { value_type, value }),
            values: BTreeMap::new(),
        }
    }

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

    #[test]
    fn ordered_scalar_admission_is_canonical_and_structured() {
        let runtime = ordered_runtime();
        let score = type_key(TypeKind::Attribute, "score");
        let overflow = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9223372036854775808".into()),
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(score, overflow)
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "invalid_input");
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");

        let double = type_key(TypeKind::Attribute, "val-double");
        let nonfinite = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Double,
            value: Value::Null,
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(double, nonfinite)
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");

        let datetime = type_key(TypeKind::Attribute, "val-datetime");
        runtime
            .validate_attribute_value_json(
                datetime,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTime,
                    value: Value::String("2026-07-29T01:02:03.120".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let datetime_tz = type_key(TypeKind::Attribute, "val-datetime-tz");
        runtime
            .validate_attribute_value_json(
                datetime_tz.clone(),
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTimeTz,
                    value: Value::String("2026-07-29T01:02:03.120Z".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let error = runtime
            .validate_attribute_value_json(
                datetime_tz,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::DateTimeTz,
                    value: Value::String("2026-07-29T01:02:03.1200Z".into()),
                })
                .unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "wrong_scalar_domain");
    }

    #[test]
    fn unordered_scalar_admission_retains_the_legacy_plain_error_contract() {
        let runtime = runtime_for_schema(UNORDERED_SCHEMA, "node-legacy-admission.yaml");
        let score = type_key(TypeKind::Attribute, "score");
        let overflow = serde_json::to_string(&ScalarWire {
            value_type: ValueTypeTag::Long,
            value: Value::String("9223372036854775808".into()),
        })
        .unwrap();
        let error = runtime
            .validate_attribute_value_json(score, overflow)
            .unwrap_err();
        assert_eq!(error.reason, "long envelope is outside i64");
        assert!(serde_json::from_str::<Value>(&error.reason).is_err());
    }

    #[test]
    fn ordered_whole_create_rejects_duplicates_and_accepts_inherited_players() {
        let runtime = ordered_runtime();
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let person_key = type_key(TypeKind::Entity, "person");
        let duplicate_person = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                (
                    "tag".into(),
                    Value::Array(
                        [
                            serde_json::to_value(&tag).unwrap(),
                            serde_json::to_value(&tag).unwrap(),
                        ]
                        .into(),
                    ),
                ),
            ]),
        };
        let error = runtime
            .validate_create_json(
                person_key.clone(),
                serde_json::to_string(&duplicate_person).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
        assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");

        let out_of_range_person = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                (
                    "score".into(),
                    serde_json::to_value(attribute_wire(
                        "score",
                        ValueTypeTag::Long,
                        Value::String("6".into()),
                    ))
                    .unwrap(),
                ),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let error = runtime
            .validate_create_json(
                person_key.clone(),
                serde_json::to_string(&out_of_range_person).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "range_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");

        let identified_person = ProjectedWire {
            type_key: person_key,
            form: WireForm::Complete,
            iid: Some("0xa".into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let plain_activity_key = type_key(TypeKind::Relation, "plain-activity");
        let plain_activity = ProjectedWire {
            type_key: plain_activity_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "participant".into(),
                serde_json::to_value(&identified_person).unwrap(),
            )]),
        };
        runtime
            .validate_create_json(
                plain_activity_key,
                serde_json::to_string(&plain_activity).unwrap(),
            )
            .unwrap();

        let membership_key = type_key(TypeKind::Relation, "membership");
        let membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                Value::Array(
                    [
                        serde_json::to_value(&identified_person).unwrap(),
                        serde_json::to_value(&identified_person).unwrap(),
                    ]
                    .into(),
                ),
            )]),
        };
        let error = runtime
            .validate_create_json(
                membership_key.clone(),
                serde_json::to_string(&membership).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");

        let unidentified_person = ProjectedWire {
            type_key: type_key(TypeKind::Entity, "person"),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([("tag".into(), Value::Array(Vec::new()))]),
        };
        let unidentified_membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                Value::Array(vec![serde_json::to_value(unidentified_person).unwrap()]),
            )]),
        };
        let error = runtime
            .validate_create_json(
                membership_key,
                serde_json::to_string(&unidentified_membership).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["code"], "missing_reference_identity");
        assert_eq!(
            diagnostic["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|segment| segment["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["type", "role", "index", "type"]
        );
    }

    #[test]
    fn ordered_facade_proof_restoration_requires_exact_visible_iid_and_keys() {
        let runtime = ordered_runtime();
        let package = runtime.package.as_ref();
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let membership_id = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let membership = package
            .projection
            .projection()
            .models()
            .get(&membership_id)
            .unwrap();
        let (role_id, role) = membership.create().roles().iter().next().unwrap();
        let operation_path = [
            SdkDiagnosticPathSegment::Type(membership_id),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
            SdkDiagnosticPathSegment::Index(0),
        ];
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let person = ProjectedWire {
            type_key: canonical_type_key(&person_id).unwrap(),
            form: WireForm::Complete,
            iid: Some("0xa1".into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let visible =
            project_reference_wire(package, role.players(), &person, &operation_path).unwrap();
        assert!(visible.origin_carrier().is_none());

        let reference_proof = NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Reference(Arc::new(visible.clone())),
        };
        let restored = project_reference_wire_with_proof(
            package,
            role.players(),
            &person,
            &operation_path,
            Some(&reference_proof),
        )
        .unwrap();
        assert_eq!(restored, visible);

        let complete = project_thing_wire(package, &person_id, &person).unwrap();
        let complete_proof = NodeProjectedFacadeProof {
            proof: FacadeProjectionProof::Thing(Arc::new(complete)),
        };
        let restored = project_reference_wire_with_proof(
            package,
            role.players(),
            &person,
            &operation_path,
            Some(&complete_proof),
        )
        .unwrap();
        assert_eq!(restored, visible);

        let mut changed_iid = person.clone();
        changed_iid.iid = Some("0xa2".into());
        let error = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_iid,
            &operation_path,
            Some(&complete_proof),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "malformed_projected_create");

        let mut changed_key = person;
        changed_key.values.insert(
            "identifier".into(),
            serde_json::to_value(attribute_wire(
                "identifier",
                ValueTypeTag::String,
                Value::String("person-2".into()),
            ))
            .unwrap(),
        );
        let error = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_key,
            &operation_path,
            Some(&reference_proof),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "malformed_projected_create");

        let lookalike = project_reference_wire_with_proof(
            package,
            role.players(),
            &changed_key,
            &operation_path,
            None,
        )
        .unwrap();
        assert_ne!(lookalike, visible);
        assert!(lookalike.origin_carrier().is_none());
    }

    #[test]
    fn ordered_single_writes_validate_before_iid_and_execution_target_resolution() {
        let runtime = ordered_runtime();
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let person_key = canonical_type_key(&person_id).unwrap();
        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let duplicate = ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(&score).unwrap()),
                (
                    "tag".into(),
                    Value::Array(vec![
                        serde_json::to_value(&tag).unwrap(),
                        serde_json::to_value(&tag).unwrap(),
                    ]),
                ),
            ]),
        };
        let duplicate_json = serde_json::to_string(&duplicate).unwrap();
        let manager = manager_without_execution_target(&runtime, person_id);

        for error in [
            manager.insert_json(duplicate_json.clone()).unwrap_err(),
            manager.put_json(duplicate_json.clone()).unwrap_err(),
            manager
                .update_json(String::new(), duplicate_json)
                .unwrap_err(),
        ] {
            let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
            assert_eq!(diagnostic["sdkCategory"], "invalid_input");
            assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
            assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
            assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");
        }

        let valid = ProjectedWire {
            type_key: person_key,
            form: WireForm::Complete,
            iid: None,
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), Value::Array(Vec::new())),
            ]),
        };
        let error = manager
            .insert_json(serde_json::to_string(&valid).unwrap())
            .unwrap_err();
        assert_eq!(error.reason, "projected manager has no execution target");
    }

    #[test]
    fn unordered_projected_read_rejects_before_execution_target_resolution() {
        let runtime = runtime_for_schema(UNORDERED_SCHEMA, "node-legacy-read.yaml");
        let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let manager = manager_without_execution_target(&runtime, person_id);

        let error = match manager.get_by_iid_projected("0xa1".into()) {
            Err(error) => error,
            Ok(_) => panic!("unordered projected read must fail"),
        };

        assert_eq!(
            error.reason,
            "common projected execution is reserved for the ordered successor runtime"
        );
    }

    #[test]
    fn ordered_hydration_uses_common_integrity_validation_for_fields_and_roles() {
        let runtime = ordered_runtime();
        let tag_key = type_key(TypeKind::Attribute, "tag");
        runtime
            .validate_hydrated_attribute_value_json(
                tag_key.clone(),
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::String,
                    value: Value::String("same".into()),
                })
                .unwrap(),
            )
            .unwrap();
        let error = runtime
            .validate_hydrated_attribute_value_json(
                tag_key,
                serde_json::to_string(&ScalarWire {
                    value_type: ValueTypeTag::String,
                    value: Value::String("INVALID".into()),
                })
                .unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "regex_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");

        let identifier = attribute_wire(
            "identifier",
            ValueTypeTag::String,
            Value::String("person-1".into()),
        );
        let score = attribute_wire("score", ValueTypeTag::Long, Value::String("3".into()));
        let tag = attribute_wire("tag", ValueTypeTag::String, Value::String("same".into()));
        let person_key = type_key(TypeKind::Entity, "person");
        let person = |iid: &str, score: ProjectedWire, tags: Vec<ProjectedWire>| ProjectedWire {
            type_key: person_key.clone(),
            form: WireForm::Complete,
            iid: Some(iid.into()),
            value: None,
            values: BTreeMap::from([
                (
                    "identifier".into(),
                    serde_json::to_value(&identifier).unwrap(),
                ),
                ("score".into(), serde_json::to_value(score).unwrap()),
                ("tag".into(), serde_json::to_value(tags).unwrap()),
            ]),
        };

        let duplicate = person("0xa1", score.clone(), vec![tag.clone(), tag]);
        let error = runtime
            .validate_thing_json(
                person_key.clone(),
                serde_json::to_string(&duplicate).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");
        assert_eq!(diagnostic["path"][2]["kind"], "index");
        assert_eq!(diagnostic["details"]["first_index"]["value"], "0");
        assert_eq!(diagnostic["details"]["duplicate_index"]["value"], "1");

        let out_of_range = person(
            "0xa2",
            attribute_wire("score", ValueTypeTag::Long, Value::String("6".into())),
            Vec::new(),
        );
        let error = runtime
            .validate_thing_json(
                person_key.clone(),
                serde_json::to_string(&out_of_range).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "range_constraint_violation");
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "field");

        let reference = person("0xa1", score, Vec::new());
        let membership_key = type_key(TypeKind::Relation, "membership");
        let membership = ProjectedWire {
            type_key: membership_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xb1".into()),
            value: None,
            values: BTreeMap::from([(
                "member".into(),
                serde_json::to_value([reference.clone(), reference]).unwrap(),
            )]),
        };
        let error = runtime
            .validate_thing_json(membership_key, serde_json::to_string(&membership).unwrap())
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "ordered_distinct_duplicate");
        assert_eq!(diagnostic["path"][1]["kind"], "role");
        assert_eq!(diagnostic["path"][2]["kind"], "index");
    }

    #[test]
    fn ordered_hydration_accepts_effective_and_polymorphic_members_and_fences_iids() {
        let runtime = ordered_runtime();
        let group_id = TypeId::new(TypeKind::Entity, "group").unwrap();
        let group_key = canonical_type_key(&group_id).unwrap();
        let group = ProjectedWire {
            type_key: group_key,
            form: WireForm::Complete,
            iid: Some("0xc1".into()),
            value: None,
            values: BTreeMap::from([("tag".into(), Value::Array(Vec::new()))]),
        };
        let activity_key = type_key(TypeKind::Relation, "plain-activity");
        let activity = ProjectedWire {
            type_key: activity_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xd1".into()),
            value: None,
            values: BTreeMap::from([("participant".into(), serde_json::to_value(group).unwrap())]),
        };
        runtime
            .validate_thing_json(
                activity_key.clone(),
                serde_json::to_string(&activity).unwrap(),
            )
            .unwrap();

        let activity_reference = ProjectedWire {
            type_key: activity_key,
            form: WireForm::Reference,
            iid: Some("0xd1".into()),
            value: None,
            values: BTreeMap::new(),
        };
        let link_key = type_key(TypeKind::Relation, "activity-link");
        let link = ProjectedWire {
            type_key: link_key.clone(),
            form: WireForm::Complete,
            iid: Some("0xe1".into()),
            value: None,
            values: BTreeMap::from([(
                "subject".into(),
                serde_json::to_value([activity_reference]).unwrap(),
            )]),
        };
        runtime
            .validate_thing_json(link_key, serde_json::to_string(&link).unwrap())
            .unwrap();

        let invalid = ProjectedWire {
            iid: Some("group-c1".into()),
            ..activity
        };
        let error = runtime
            .validate_thing_json(
                type_key(TypeKind::Relation, "plain-activity"),
                serde_json::to_string(&invalid).unwrap(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "noncanonical_hydrated_iid");
        assert_eq!(diagnostic["path"][1]["kind"], "argument");
    }

    #[test]
    fn foreign_member_diagnostic_has_fixed_message_and_operation_path() {
        let runtime = ordered_runtime();
        let membership = type_key(TypeKind::Relation, "membership");
        let error = runtime
            .reject_generated_token_package_mismatch(
                serde_json::json!([
                    {"kind": "type", "typeKey": membership},
                    {"kind": "role", "name": "member"},
                    {"kind": "index", "value": 1},
                ])
                .to_string(),
            )
            .unwrap_err();
        let diagnostic: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(diagnostic["category"], "integrity");
        assert_eq!(diagnostic["sdkCategory"], "integrity");
        assert_eq!(diagnostic["code"], "generated_token_package_mismatch");
        assert_eq!(
            diagnostic["message"],
            "The generated token belongs to a different installed schema package"
        );
        assert_eq!(diagnostic["path"][0]["kind"], "type");
        assert_eq!(diagnostic["path"][1]["kind"], "role");
        assert_eq!(
            diagnostic["path"][2],
            serde_json::json!({"kind": "index", "value": 1})
        );
    }

    #[test]
    fn authorityless_install_retains_exact_unordered_legacy_admission_errors() {
        let legacy_authority = authority(UNORDERED_SCHEMA, "node-legacy-install.yaml");
        let legacy = typescript_projection(&legacy_authority);
        let (legacy_json, legacy_semantic, legacy_fingerprint) = evidence(&legacy);
        NodeRuntimeProjection::new(
            legacy_json.clone(),
            legacy_semantic.clone(),
            legacy_fingerprint.clone(),
            registrations(&legacy),
            None,
        )
        .expect("exact legacy unordered evidence must remain authorityless");

        let malformed = NodeRuntimeProjection::new(
            "{".into(),
            legacy_semantic,
            legacy_fingerprint,
            "[]".into(),
            None,
        )
        .err()
        .expect("malformed legacy projection evidence must fail");
        assert_eq!(
            malformed.reason,
            "invalid_contract [malformed_canonical_json]: input is not valid canonical JSON"
        );

        let emitter = TypeScriptEmitter::new();
        let mut forged_resources = emitter.code_resources().unwrap();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged legacy resource").unwrap();
        let forged = project(
            legacy_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers(),
            &forged_resources,
        )
        .unwrap();
        let (projection, semantic, fingerprint) = evidence(&forged);
        let forged =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None)
                .err()
                .expect("forged legacy resource evidence must fail");
        assert_eq!(
            forged.reason,
            "legacy TypeScript runtime projection does not match the exact shipped handler and resource evidence"
        );

        let ordered_authority = authority(ORDERED_SCHEMA, "node-legacy-ordered-install.yaml");
        let ordered = typescript_projection(&ordered_authority);
        let (projection, semantic, fingerprint) = evidence(&ordered);
        let missing_authority =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None)
                .err()
                .expect("authorityless ordered projection evidence must fail");
        assert_projection_evidence_mismatch(missing_authority);

        let authority = authority(ORDERED_SCHEMA, "node-foreign-target.yaml");
        let emitter = PythonEmitter::new();
        let projection = project(
            authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &emitter.generator_handlers_for(authority.resolved_schema()),
            &emitter
                .code_resources_for(authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let (projection, semantic, fingerprint) = evidence(&projection);
        let error =
            NodeRuntimeProjection::new(projection, semantic, fingerprint, "[]".into(), None)
                .err()
                .expect("a Python projection must not install as TypeScript");
        assert_eq!(
            error.reason,
            "runtime projection does not target TypeScript"
        );
    }

    #[test]
    fn successor_install_converges_all_hostile_evidence_on_the_integrity_diagnostic() {
        let ordered_authority = authority(ORDERED_SCHEMA, "node-ordered.yaml");
        let authority_json =
            String::from_utf8(encode_schema_authority(&ordered_authority)).unwrap();
        let exact = typescript_projection(&ordered_authority);
        let emitter = TypeScriptEmitter::new();
        assert_eq!(
            exact.generator_handlers(),
            [ProjectionHandler::typescript_v2()]
        );

        let (exact_json, exact_semantic, exact_fingerprint) = evidence(&exact);
        NodeRuntimeProjection::new(
            exact_json.clone(),
            exact_semantic.clone(),
            exact_fingerprint.clone(),
            registrations(&exact),
            Some(authority_json.clone()),
        )
        .expect("the exact ordered TypeScript package evidence must install");

        let rejected = |projection_json: String,
                        semantic: String,
                        fingerprint: String,
                        authority_json: Option<&str>| {
            NodeRuntimeProjection::new(
                projection_json,
                semantic,
                fingerprint,
                "[]".into(),
                authority_json.map(str::to_owned),
            )
            .err()
            .expect("hostile ordered projection evidence must fail")
        };

        let mut missing_resources = exact.code_resources().to_vec();
        missing_resources.pop();
        let missing = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &[ProjectionHandler::typescript_v2()],
            &missing_resources,
        )
        .unwrap();

        let mut raw: Value = serde_json::from_str(&exact_json).unwrap();
        let resources = raw["code_resources"].as_array().unwrap();
        let first_resource = resources[0].clone();

        let mut extra: Value = serde_json::from_str(&exact_json).unwrap();
        let mut extra_resource = first_resource.clone();
        extra_resource["id"] =
            Value::String("typebridge.generator.typescript.zzz-extra-resource".to_owned());
        extra["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(extra_resource);
        let extra = String::from_utf8(to_canonical_json(&extra).unwrap()).unwrap();

        let mut duplicate: Value = serde_json::from_str(&exact_json).unwrap();
        duplicate["code_resources"]
            .as_array_mut()
            .unwrap()
            .push(first_resource);
        let duplicate = String::from_utf8(to_canonical_json(&duplicate).unwrap()).unwrap();

        let stale = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();

        let mut forged_resources = exact.code_resources().to_vec();
        let forged_id = forged_resources[0].id().as_str().to_owned();
        forged_resources[0] =
            CodeResourceDigest::from_bytes(forged_id, b"forged TypeScript resource").unwrap();
        let forged = project(
            ordered_authority.resolved_schema(),
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &[ProjectionHandler::typescript_v2()],
            &forged_resources,
        )
        .unwrap();

        raw["code_resources"].as_array_mut().unwrap().reverse();
        let reordered = String::from_utf8(to_canonical_json(&raw).unwrap()).unwrap();

        let foreign = authority(
            "format: typebridge.schema/v2\nentities:\n  foreign: {}\n",
            "node-foreign.yaml",
        );
        let foreign = String::from_utf8(encode_schema_authority(&foreign)).unwrap();

        let python = PythonEmitter::new();
        let foreign_target = project(
            ordered_authority.resolved_schema(),
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &python.generator_handlers_for(ordered_authority.resolved_schema()),
            &python
                .code_resources_for(ordered_authority.resolved_schema())
                .unwrap(),
        )
        .unwrap();
        let (foreign_target, foreign_target_semantic, foreign_target_fingerprint) =
            evidence(&foreign_target);

        let (missing, missing_semantic, missing_fingerprint) = evidence(&missing);
        let (stale, stale_semantic, stale_fingerprint) = evidence(&stale);
        let (forged, forged_semantic, forged_fingerprint) = evidence(&forged);

        for error in [
            rejected(
                "{".into(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                "{".into(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                "{".into(),
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some("{"),
            ),
            rejected(
                missing,
                missing_semantic,
                missing_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                extra,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                duplicate,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                reordered,
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&authority_json),
            ),
            rejected(
                stale,
                stale_semantic,
                stale_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                forged,
                forged_semantic,
                forged_fingerprint,
                Some(&authority_json),
            ),
            rejected(
                exact_json.clone(),
                exact_semantic.clone(),
                exact_fingerprint.clone(),
                Some(&foreign),
            ),
            rejected(
                foreign_target,
                foreign_target_semantic,
                foreign_target_fingerprint,
                Some(&authority_json),
            ),
        ] {
            assert_projection_evidence_mismatch(error);
        }
    }
}
