#![deny(missing_docs)]
//! Owner-branded query sessions, bindings, predicates, and the one query
//! facade (Flight 3: F3-01 foundation, F3-02 algebra, F3-03 singular shapes).

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::decimal::parse_decimal;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::query_plan::CompatibilityValueV2;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_orm::match_request::handles::{
    BindingHandle as OrmBindingHandle, FieldHandle as OrmFieldHandle,
    OrderHandle as OrmOrderHandle, PredicateHandle as OrmPredicateHandle,
    QueryHandle as OrmQueryHandle, SelectionHandle as OrmSelectionHandle,
    SessionHandle as OrmSessionHandle, ShapeHandle as OrmShapeHandle,
};
use type_bridge_orm::match_request::model::{
    ComparisonOp, MissingOrder, RowCardinality, SortDirection, Window,
};
use type_bridge_orm::match_request::result::{
    HydratedThing, MatchResult, MatchRow, SlotValue, ValidatedMatchResult,
};
use type_bridge_orm::match_request::validation::ValidatedMatchRequest;
use type_bridge_orm::{
    AttributeValue, DescriptorRegistry, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    InstalledRuntimeProjection,
};

use crate::__codegen::{
    CompleteModel, EncodedScalar, FieldToken, HydratedRow, HydrationCapability, Model, QueryValued,
    RelationModel, RolePlayer, RoleToken, RoleTokenCompatible, SubtypeRootModel, ThingModel,
    TypeToken, ValidationError,
};
use crate::entity_codec::{hydrate_entity, map_validation_error};
use crate::error::{Error, ModelValidationPhase};
use crate::relation_codec::hydrate_relation;
use crate::schema::Schema;
use crate::{Database, Result};

#[cfg(test)]
mod tests;

static QUERY_SESSION_NONCE: AtomicU64 = AtomicU64::new(1);

fn schema_not_bound() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "schema_not_bound",
        vec![],
        "database is not schema-bound",
        None,
    )
}

fn cross_session_handle() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "cross_session_handle",
        vec![],
        "binding belongs to a different query session",
        None,
    )
}

fn model_label(type_id_json: &'static str) -> Result<String> {
    let id = from_canonical_json::<TypeId>(type_id_json.as_bytes()).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not canonical",
            Some(Box::new(source)),
        )
    })?;
    Ok(id.label().as_str().to_owned())
}

fn parse_owns_identity(owns_id_json: &'static str) -> Result<(String, String)> {
    let invalid = || {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_field_identity",
            vec!["type".into()],
            "generated field identity is not canonical",
            None,
        )
    };
    let value: serde_json::Value = serde_json::from_str(owns_id_json).map_err(|_| invalid())?;
    let attribute = value
        .get("attribute")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    let owner = value
        .get("owner")
        .and_then(|owner| owner.get("label"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    Ok((owner.to_owned(), attribute.to_owned()))
}

fn parse_role_identity(role_id_json: &'static str) -> Result<type_bridge_contract::id::RoleId> {
    from_canonical_json::<type_bridge_contract::id::RoleId>(role_id_json.as_bytes()).map_err(
        |source| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_role_identity",
                vec!["type".into()],
                "generated role identity is not canonical",
                Some(Box::new(source)),
            )
        },
    )
}

mod mode_sealed {
    pub trait Sealed {}
}

/// Sealed marker for a binding's exact-versus-subtypes selection behavior.
pub trait SelectionMode: mode_sealed::Sealed + 'static {}

/// Exact-match selection: results materialize as the bound concrete model.
#[derive(Clone, Copy, Debug)]
pub struct Exact;
impl mode_sealed::Sealed for Exact {}
impl SelectionMode for Exact {}

/// Subtype-inclusive selection: results materialize as the generated leaf or
/// closed family associated with the bound root.
#[derive(Clone, Copy, Debug)]
pub struct Subtypes;
impl mode_sealed::Sealed for Subtypes {}
impl SelectionMode for Subtypes {}

/// One isolated owner-branded query authoring session.
///
/// Every binding call allocates a fresh binding identity; bindings are
/// lightweight `Copy` values valid only for the session that created them.
/// Bindings from another session fail before I/O with
/// `cross_session_handle`.
pub struct QuerySession<'db, S: Schema> {
    installed: &'db InstalledRuntimeProjection,
    execution: QueryExecution<'db, S>,
    session: OrmSessionHandle,
    registry: Arc<DescriptorRegistry>,
    nonce: u64,
    bindings: Vec<OrmBindingHandle>,
    marker: PhantomData<fn() -> S>,
}

enum QueryExecution<'db, S: Schema> {
    Local(&'db Database<S>),
    Borrowed(&'db type_bridge_orm::session::context::TransactionContext),
    Remote(&'db crate::remote::RemoteDatabase<S>),
}

impl<S: Schema> std::fmt::Debug for QuerySession<'_, S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QuerySession")
            .field("session", &self.nonce)
            .field("bindings", &self.bindings.len())
            .finish_non_exhaustive()
    }
}

/// Opaque session-scoped binding identity carried by `Copy` handles.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingKey {
    pub(crate) nonce: u64,
    pub(crate) index: u32,
}

/// One schema/model-branded `Copy` binding token allocated by a
/// [`QuerySession`]. Copying preserves the binding identity; only a fresh
/// session binding call creates a new occurrence.
pub struct Binding<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode = Exact> {
    key: BindingKey,
    #[allow(clippy::type_complexity)]
    marker: PhantomData<fn() -> (S, M, Mode)>,
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Copy for Binding<S, M, Mode> {}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Clone for Binding<S, M, Mode> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> PartialEq for Binding<S, M, Mode> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Eq for Binding<S, M, Mode> {}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> std::fmt::Debug
    for Binding<S, M, Mode>
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Binding")
            .field("session", &self.key.nonce)
            .field("index", &self.key.index)
            .finish()
    }
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Binding<S, M, Mode> {
    pub(crate) fn key(self) -> BindingKey {
        self.key
    }

    /// Select this binding as an owned collection per distinct page root,
    /// preserving match multiplicity by default.
    #[must_use]
    pub fn collect(self) -> Collected<S, Self>
    where
        Self: Selectable<S>,
    {
        Collected {
            selection: self,
            distinct: false,
            order: Vec::new(),
        }
    }
}

impl<S: Schema> Database<S> {
    /// Start one owner-branded query authoring session over this
    /// schema-bound database.
    pub fn query(&self) -> Result<QuerySession<'_, S>> {
        let registry = self.match_registry().ok_or_else(schema_not_bound)?;
        let installed = self
            .installed_schema()
            .map(Arc::as_ref)
            .ok_or_else(schema_not_bound)?;
        Ok(QuerySession::new(
            installed,
            Arc::clone(registry),
            QueryExecution::Local(self),
        ))
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    fn new(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        execution: QueryExecution<'db, S>,
    ) -> Self {
        Self {
            installed,
            execution,
            session: OrmSessionHandle::new(Arc::clone(&registry)),
            registry,
            nonce: QUERY_SESSION_NONCE.fetch_add(1, Ordering::Relaxed),
            bindings: Vec::new(),
            marker: PhantomData,
        }
    }

    pub(crate) fn borrowed(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        transaction: &'db type_bridge_orm::session::context::TransactionContext,
    ) -> Self {
        Self::new(installed, registry, QueryExecution::Borrowed(transaction))
    }

    pub(crate) fn remote(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        remote: &'db crate::remote::RemoteDatabase<S>,
    ) -> Self {
        Self::new(installed, registry, QueryExecution::Remote(remote))
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    fn push_binding<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &mut self,
        handle: OrmBindingHandle,
    ) -> Result<Binding<S, M, Mode>> {
        let index = u32::try_from(self.bindings.len()).map_err(|source| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "too_many_bindings",
                vec![],
                "query session binding capacity exceeded",
                Some(Box::new(source)),
            )
        })?;
        self.bindings.push(handle);
        Ok(Binding {
            key: BindingKey {
                nonce: self.nonce,
                index,
            },
            marker: PhantomData,
        })
    }

    /// Allocate a fresh exact-match binding for one concrete complete
    /// generated model. Abstract models have no exact binding constructor.
    pub fn exact<M>(&mut self) -> Result<Binding<S, M, Exact>>
    where
        M: ThingModel<Schema = S> + CompleteModel,
    {
        let label = model_label(M::TYPE_ID_JSON)?;
        let handle = self.session.exact(&label).map_err(Error::from_orm)?;
        self.push_binding(handle)
    }

    /// Allocate a fresh subtype-inclusive binding for one generated subtype
    /// root; results materialize as the generated leaf or closed family.
    pub fn subtypes<M>(&mut self) -> Result<Binding<S, M, Subtypes>>
    where
        M: ThingModel<Schema = S> + SubtypeRootModel,
    {
        let label = model_label(M::TYPE_ID_JSON)?;
        let handle = self.session.subtypes(&label).map_err(Error::from_orm)?;
        self.push_binding(handle)
    }

    pub(crate) fn handle_by_key(&self, key: BindingKey) -> Result<&OrmBindingHandle> {
        if key.nonce != self.nonce {
            return Err(cross_session_handle());
        }
        self.bindings
            .get(key.index as usize)
            .ok_or_else(cross_session_handle)
    }

    fn installed(&self) -> Result<&InstalledRuntimeProjection> {
        Ok(self.installed)
    }

    fn field_name_for(&self, owner_label: &str, attribute_label: &str) -> Result<String> {
        let descriptor = self.registry.get(owner_label).ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "unknown_field_owner",
                vec!["type".into()],
                format!("field owner '{owner_label}' is not registered in this session"),
                None,
            )
        })?;
        let found = match &descriptor {
            type_bridge_orm::TypeDescriptorRef::Entity(entity) => entity
                .owned_attributes
                .iter()
                .find(|attribute| attribute.attr_name == attribute_label)
                .map(|attribute| attribute.field_name.clone()),
            type_bridge_orm::TypeDescriptorRef::Relation(relation) => relation
                .owned_attributes
                .iter()
                .find(|attribute| attribute.attr_name == attribute_label)
                .map(|attribute| attribute.field_name.clone()),
        };
        found.ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "unknown_field",
                vec!["type".into()],
                format!("owner '{owner_label}' has no field for attribute '{attribute_label}'"),
                None,
            )
        })
    }

    pub(crate) fn lower_field(
        &self,
        key: BindingKey,
        owns_id_json: &'static str,
    ) -> Result<OrmFieldHandle> {
        let handle = self.handle_by_key(key)?;
        let (owner_label, attribute_label) = parse_owns_identity(owns_id_json)?;
        let field_name = self.field_name_for(&owner_label, &attribute_label)?;
        handle
            .field_owned_by(&owner_label, &field_name)
            .map_err(Error::from_orm)
    }

    fn lower_order(&self, order: &Order<S>) -> Result<OrmOrderHandle> {
        let field = self.lower_field(order.key, order.owns_id_json)?;
        Ok(field.order(order.direction, order.missing))
    }

    fn lower_predicate(&self, expr: &PredicateExpr) -> Result<OrmPredicateHandle> {
        match expr {
            PredicateExpr::FieldValue {
                binding,
                owns_id_json,
                operator,
                value,
            } => {
                let field = self.lower_field(*binding, owns_id_json)?;
                Ok(field.compare_value(*operator, value.clone()))
            }
            PredicateExpr::FieldField {
                left_binding,
                left_owns_id_json,
                operator,
                right_binding,
                right_owns_id_json,
            } => {
                let left = self.lower_field(*left_binding, left_owns_id_json)?;
                let right = self.lower_field(*right_binding, right_owns_id_json)?;
                left.compare_field(*operator, &right)
                    .map_err(Error::from_orm)
            }
            PredicateExpr::Connects {
                relation,
                role_id_json,
                player,
            } => {
                let relation_handle = self.handle_by_key(*relation)?;
                let role = parse_role_identity(role_id_json)?;
                let role_handle = relation_handle
                    .role_owned_by(role.declaring_relation().as_str(), role.label().as_str())
                    .map_err(Error::from_orm)?;
                let player_handle = self.handle_by_key(*player)?;
                role_handle.connects(player_handle).map_err(Error::from_orm)
            }
            PredicateExpr::Reachable {
                relation_type_id_json,
                role_from_id_json,
                role_to_id_json,
                source,
                target,
                min_depth,
                max_depth,
            } => {
                let relation = model_label(relation_type_id_json)?;
                let role_from = parse_role_identity(role_from_id_json)?;
                let role_to = parse_role_identity(role_to_id_json)?;
                self.session
                    .reachable(
                        &relation,
                        role_from.label().as_str(),
                        role_to.label().as_str(),
                        self.handle_by_key(*source)?,
                        self.handle_by_key(*target)?,
                        *min_depth,
                        *max_depth,
                    )
                    .map_err(Error::from_orm)
            }
            PredicateExpr::And(terms) => self.lower_composed(terms, |left, right| {
                left.and(right).map_err(Error::from_orm)
            }),
            PredicateExpr::Or(terms) => {
                self.lower_composed(terms, |left, right| left.or(right).map_err(Error::from_orm))
            }
            PredicateExpr::Not(inner) => Ok(self.lower_predicate(inner)?.not()),
        }
    }

    fn lower_composed(
        &self,
        terms: &[PredicateExpr],
        combine: impl Fn(&OrmPredicateHandle, &OrmPredicateHandle) -> Result<OrmPredicateHandle>,
    ) -> Result<OrmPredicateHandle> {
        let mut lowered = terms.iter().map(|term| self.lower_predicate(term));
        let mut combined = lowered.next().ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "empty_predicate",
                vec![],
                "boolean composition requires at least one predicate",
                None,
            )
        })??;
        for term in lowered {
            combined = combine(&combined, &term?)?;
        }
        Ok(combined)
    }

    fn client_row_for(&self, thing: &HydratedThing) -> Result<HydratedRow> {
        use type_bridge_orm::match_request::model::ThingKind as OrmThingKind;
        let installed = self.installed()?;
        let type_name = self
            .registry
            .descriptor_type_name(thing.concrete_descriptor())
            .ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "invalid_installed_projection",
                    vec!["type".into()],
                    "selected concrete descriptor is absent from the installed registry",
                    None,
                )
            })?;
        let mut attributes = Vec::new();
        for attribute in thing.attributes() {
            let provider_name = self
                .registry
                .provider_attribute_name(attribute.field())
                .ok_or_else(|| {
                    Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "invalid_installed_projection",
                        vec![],
                        "selected field identity has no provider attribute name",
                        None,
                    )
                })?;
            for value in attribute.values() {
                attributes.push((
                    provider_name.clone(),
                    canonicalize_selected_value(value.clone())?,
                ));
            }
        }
        match thing.kind() {
            OrmThingKind::Entity => {
                let id = TypeId::new(type_bridge_contract::id::TypeKind::Entity, &type_name)
                    .map_err(|source| {
                        Error::model_validation(
                            ModelValidationPhase::Hydration,
                            "invalid_discovered_type",
                            vec!["type".into()],
                            "selected entity type label is invalid",
                            Some(Box::new(source)),
                        )
                    })?;
                hydrate_entity(
                    DynamicEntityRow {
                        iid: Some(thing.concept_id().as_str().to_owned()),
                        type_name: Some(type_name),
                        attributes,
                    },
                    &id,
                    installed,
                )
            }
            OrmThingKind::Relation => {
                let id = TypeId::new(type_bridge_contract::id::TypeKind::Relation, &type_name)
                    .map_err(|source| {
                        Error::model_validation(
                            ModelValidationPhase::Hydration,
                            "invalid_discovered_type",
                            vec!["type".into()],
                            "selected relation type label is invalid",
                            Some(Box::new(source)),
                        )
                    })?;
                let mut role_players = Vec::new();
                for role in thing.roles() {
                    for player in role.players() {
                        let mut raw = Vec::new();
                        for attribute in player.attributes() {
                            let provider_name = self
                                .registry
                                .provider_attribute_name(attribute.field())
                                .ok_or_else(|| {
                                    Error::model_validation(
                                        ModelValidationPhase::Hydration,
                                        "invalid_installed_projection",
                                        vec![],
                                        "player field identity has no provider attribute name",
                                        None,
                                    )
                                })?;
                            for value in attribute.values() {
                                raw.push((
                                    provider_name.clone(),
                                    plain_json(&canonicalize_selected_value(value.clone())?),
                                ));
                            }
                        }
                        let player_type_name = self
                            .registry
                            .descriptor_type_name(player.concrete_descriptor())
                            .ok_or_else(|| {
                                Error::model_validation(
                                    ModelValidationPhase::Hydration,
                                    "invalid_installed_projection",
                                    vec!["roles".into(), role.role().name.clone()],
                                    "selected role-player descriptor is absent from the installed registry",
                                    None,
                                )
                            })?;
                        role_players.push(DynamicRolePlayer {
                            role_name: role.role().name.clone(),
                            player_iid: Some(player.concept_id().as_str().to_owned()),
                            player_type_name: Some(player_type_name),
                            attributes: raw,
                        });
                    }
                }
                hydrate_relation(
                    DynamicRelationRow {
                        iid: Some(thing.concept_id().as_str().to_owned()),
                        type_name: Some(type_name),
                        attributes,
                        role_players,
                    },
                    &id,
                    installed,
                )
            }
        }
    }
}

fn canonicalize_selected_value(value: AttributeValue) -> Result<AttributeValue> {
    let malformed = || {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "hydrated_attribute_value_type",
            vec![],
            "selected attribute value is outside its canonical scalar domain",
            None,
        )
    };
    match value {
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(|value| AttributeValue::Date(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTime(value) => normalize_provider_fraction(value)
            .parse::<CanonicalDateTime>()
            .map(|value| AttributeValue::DateTime(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTimeTZ(value) => normalize_provider_datetime_tz(value)
            .parse::<CanonicalDateTimeTz>()
            .map(|value| AttributeValue::DateTimeTZ(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::Decimal(value) => parse_decimal(&value)
            .map(|value| AttributeValue::Decimal(value.canonical_string()))
            .ok_or_else(malformed),
        AttributeValue::Duration(value) => {
            let value = normalize_provider_fraction(value);
            match value.parse::<CanonicalDuration>() {
                Ok(value) => Ok(AttributeValue::Duration(value.to_string())),
                Err(_) => CompatibilityValueV2::released_duration(value.clone())
                    .map(|_| AttributeValue::Duration(value))
                    .map_err(|_| malformed()),
            }
        }
        value => Ok(value),
    }
}

fn normalize_provider_datetime_tz(value: String) -> String {
    let mut normalized = normalize_provider_fraction(value);
    for zero_offset in ["+00:00:00", "-00:00:00", "+00:00", "-00:00"] {
        if normalized.ends_with(zero_offset) {
            normalized.truncate(normalized.len() - zero_offset.len());
            normalized.push('Z');
            break;
        }
    }
    normalized
}

fn normalize_provider_fraction(value: String) -> String {
    let Some(dot) = value.find('.') else {
        return value;
    };
    let fraction_end = value[dot + 1..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(value.len(), |offset| dot + 1 + offset);
    let trimmed_end = value[dot + 1..fraction_end].trim_end_matches('0').len() + dot + 1;
    if trimmed_end == fraction_end {
        return value;
    }
    let mut normalized = String::with_capacity(value.len());
    normalized.push_str(
        &value[..if trimmed_end == dot + 1 {
            dot
        } else {
            trimmed_end
        }],
    );
    normalized.push_str(&value[fraction_end..]);
    normalized
}

fn plain_json(value: &AttributeValue) -> serde_json::Value {
    match value {
        AttributeValue::String(value) => serde_json::Value::String(value.clone()),
        AttributeValue::Long(value) => serde_json::Value::from(*value),
        AttributeValue::Double(value) => {
            serde_json::Number::from_f64(*value).map_or(serde_json::Value::Null, Into::into)
        }
        AttributeValue::Boolean(value) => serde_json::Value::Bool(*value),
        AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => serde_json::Value::String(value.clone()),
    }
}

/// Sealed conversion from client literals and generated value wrappers into
/// canonical query operands.
pub trait QueryOperand: operand_sealed::Sealed {
    #[doc(hidden)]
    type Domain;

    #[doc(hidden)]
    fn into_operand(self) -> AttributeValue;
}

/// Sealed marker for canonically ordered query operands.
pub trait OrderedOperand: QueryOperand {}

mod operand_sealed {
    pub trait Sealed {}
}

macro_rules! operand {
    ($ty:ty, $domain:ty, $self_:ident => $convert:expr, ordered: $ordered:tt) => {
        impl operand_sealed::Sealed for $ty {}
        impl QueryOperand for $ty {
            type Domain = $domain;

            fn into_operand($self_) -> AttributeValue {
                $convert
            }
        }
        operand!(@ordered $ty, $ordered);
    };
    (@ordered $ty:ty, true) => {
        impl OrderedOperand for $ty {}
    };
    (@ordered $ty:ty, false) => {};
}

fn encoded_query_operand(value: EncodedScalar) -> AttributeValue {
    match value {
        EncodedScalar::String(value) => AttributeValue::String(value),
        EncodedScalar::Long(value) => AttributeValue::Long(value),
        EncodedScalar::Double(value) => AttributeValue::Double(value.get()),
        EncodedScalar::Boolean(value) => AttributeValue::Boolean(value),
        EncodedScalar::Date(value) => AttributeValue::Date(value.as_str().to_owned()),
        EncodedScalar::DateTime(value) => AttributeValue::DateTime(value.as_str().to_owned()),
        EncodedScalar::DateTimeTz(value) => AttributeValue::DateTimeTZ(value.as_str().to_owned()),
        EncodedScalar::Decimal(value) => AttributeValue::Decimal(value.as_str().to_owned()),
        EncodedScalar::Duration(value) => AttributeValue::Duration(value.as_str().to_owned()),
    }
}

impl<T: QueryValued> operand_sealed::Sealed for T {}
impl<T: QueryValued> QueryOperand for T {
    type Domain = T::Domain;

    fn into_operand(self) -> AttributeValue {
        encoded_query_operand(self.into_encoded_scalar())
    }
}

impl OrderedOperand for i64 {}

operand!(
    crate::value::Text,
    String,
    self => AttributeValue::String(self.into_string()),
    ordered: false
);
operand!(
    crate::value::Double,
    crate::__codegen::CanonicalDouble,
    self => AttributeValue::Double(self.get()),
    ordered: true
);
operand!(
    crate::value::Decimal,
    crate::__codegen::Decimal,
    self => AttributeValue::Decimal(self.into_string()),
    ordered: true
);
operand!(
    crate::value::Date,
    crate::__codegen::Date,
    self => AttributeValue::Date(self.into_string()),
    ordered: true
);
operand!(
    crate::value::DateTime,
    crate::__codegen::DateTime,
    self => AttributeValue::DateTime(self.into_string()),
    ordered: true
);
operand!(
    crate::value::DateTimeTz,
    crate::__codegen::DateTimeTz,
    self => AttributeValue::DateTimeTZ(self.into_string()),
    ordered: true
);
operand!(
    crate::value::Duration,
    crate::__codegen::Duration,
    self => AttributeValue::Duration(self.into_string()),
    ordered: true
);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PredicateExpr {
    FieldValue {
        binding: BindingKey,
        owns_id_json: &'static str,
        operator: ComparisonOp,
        value: AttributeValue,
    },
    FieldField {
        left_binding: BindingKey,
        left_owns_id_json: &'static str,
        operator: ComparisonOp,
        right_binding: BindingKey,
        right_owns_id_json: &'static str,
    },
    Connects {
        relation: BindingKey,
        role_id_json: &'static str,
        player: BindingKey,
    },
    Reachable {
        relation_type_id_json: &'static str,
        role_from_id_json: &'static str,
        role_to_id_json: &'static str,
        source: BindingKey,
        target: BindingKey,
        min_depth: u8,
        max_depth: u8,
    },
    And(Vec<PredicateExpr>),
    Or(Vec<PredicateExpr>),
    Not(Box<PredicateExpr>),
}

/// One schema-branded, composable query predicate.
///
/// Operators are domain-restricted at construction; predicates compose with
/// `&`, `|`, and `!` (or the named [`Predicate::and`], [`Predicate::or`],
/// and [`Predicate::not`]) and are validated against the owning session
/// before any executor invocation.
#[derive(Debug, PartialEq)]
pub struct Predicate<S: Schema> {
    pub(crate) expr: PredicateExpr,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Clone for Predicate<S> {
    fn clone(&self) -> Self {
        Self {
            expr: self.expr.clone(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema> Predicate<S> {
    fn new(expr: PredicateExpr) -> Self {
        Self {
            expr,
            marker: PhantomData,
        }
    }

    /// Conjunction; equivalent to `self & other`.
    #[must_use]
    pub fn and(self, other: Predicate<S>) -> Predicate<S> {
        self & other
    }

    /// Disjunction; equivalent to `self | other`.
    #[must_use]
    pub fn or(self, other: Predicate<S>) -> Predicate<S> {
        self | other
    }

    /// Negation; equivalent to `!self`.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Predicate<S> {
        !self
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    /// Require a bounded directed walk between two generated endpoint
    /// bindings through one exact generated relation.
    ///
    /// Each hop follows `role_from -> role_to`. Bounds are inclusive and a
    /// zero-hop branch requires identical endpoint concepts. Generated role
    /// compatibility and player-union evidence reject inactive roles and
    /// invalid endpoint models at compile time; bounds, session ownership,
    /// and installed-schema compatibility are validated before provider I/O.
    #[allow(clippy::too_many_arguments)]
    pub fn reachable<
        R,
        FromOwner,
        FromPlayers,
        ToOwner,
        ToPlayers,
        Source,
        SourceMode,
        Target,
        TargetMode,
    >(
        &self,
        relation: TypeToken<R>,
        role_from: RoleToken<FromOwner, FromPlayers>,
        role_to: RoleToken<ToOwner, ToPlayers>,
        source: Binding<S, Source, SourceMode>,
        target: Binding<S, Target, TargetMode>,
        min_depth: u8,
        max_depth: u8,
    ) -> Result<Predicate<S>>
    where
        R: RelationModel<Schema = S>
            + CompleteModel
            + RoleTokenCompatible<FromOwner, FromPlayers>
            + RoleTokenCompatible<ToOwner, ToPlayers>,
        FromOwner: RelationModel<Schema = S>,
        ToOwner: RelationModel<Schema = S>,
        FromPlayers: RolePlayer<Source>,
        ToPlayers: RolePlayer<Target>,
        Source: ThingModel<Schema = S>,
        SourceMode: SelectionMode,
        Target: ThingModel<Schema = S>,
        TargetMode: SelectionMode,
    {
        let predicate = Predicate::new(PredicateExpr::Reachable {
            relation_type_id_json: relation.type_id_json(),
            role_from_id_json: role_from.role_id_json(),
            role_to_id_json: role_to.role_id_json(),
            source: source.key,
            target: target.key,
            min_depth,
            max_depth,
        });
        self.lower_predicate(&predicate.expr)?;
        Ok(predicate)
    }
}

impl<S: Schema> std::ops::BitAnd for Predicate<S> {
    type Output = Predicate<S>;
    fn bitand(self, other: Predicate<S>) -> Predicate<S> {
        let mut terms = match self.expr {
            PredicateExpr::And(terms) => terms,
            expr => vec![expr],
        };
        match other.expr {
            PredicateExpr::And(more) => terms.extend(more),
            expr => terms.push(expr),
        }
        Predicate::new(PredicateExpr::And(terms))
    }
}

impl<S: Schema> std::ops::BitOr for Predicate<S> {
    type Output = Predicate<S>;
    fn bitor(self, other: Predicate<S>) -> Predicate<S> {
        let mut terms = match self.expr {
            PredicateExpr::Or(terms) => terms,
            expr => vec![expr],
        };
        match other.expr {
            PredicateExpr::Or(more) => terms.extend(more),
            expr => terms.push(expr),
        }
        Predicate::new(PredicateExpr::Or(terms))
    }
}

impl<S: Schema> std::ops::Not for Predicate<S> {
    type Output = Predicate<S>;
    fn not(self) -> Predicate<S> {
        Predicate::new(PredicateExpr::Not(Box::new(self.expr)))
    }
}

/// One generated field resolved against one session binding occurrence.
///
/// The token retains its declaring owner; owner/binding compatibility is
/// enforced against the installed registry when the predicate is lowered,
/// before any I/O.
pub struct BoundField<S: Schema, Owner: Model<Schema = S>, V> {
    key: BindingKey,
    owns_id_json: &'static str,
    marker: PhantomData<fn() -> (Owner, V)>,
}

impl<S: Schema, Owner: Model<Schema = S>, V> Copy for BoundField<S, Owner, V> {}
impl<S: Schema, Owner: Model<Schema = S>, V> Clone for BoundField<S, Owner, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Binding<S, M, Mode> {
    /// Resolve one generated owned field against this binding occurrence.
    ///
    /// The declaring owner must be this binding's model or a generated
    /// nominal ancestor; unrelated same-spelled owners fail to type-check,
    /// and the installed registry re-validates the admission at lowering.
    #[must_use]
    pub fn field<Owner, V>(self, token: FieldToken<Owner, V>) -> BoundField<S, Owner, V>
    where
        Owner: Model<Schema = S>,
        M: crate::__codegen::NominalUpcast<Owner>,
    {
        BoundField {
            key: self.key,
            owns_id_json: token.owns_id_json(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, M: RelationModel<Schema = S> + ThingModel<Schema = S>, Mode: SelectionMode>
    Binding<S, M, Mode>
{
    /// Resolve one active generated relation role against this relation
    /// binding occurrence. Only relation bindings expose roles;
    /// specialized-away ancestor tokens have no generated compatibility
    /// evidence.
    #[must_use]
    pub fn role<Owner, Players>(
        self,
        token: RoleToken<Owner, Players>,
    ) -> BoundRole<S, Owner, Players>
    where
        Owner: RelationModel<Schema = S>,
        M: RoleTokenCompatible<Owner, Players>,
    {
        BoundRole {
            key: self.key,
            role_id_json: token.role_id_json(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, Owner: Model<Schema = S>, V> BoundField<S, Owner, V> {
    pub(crate) fn reduction_input(self) -> (BindingKey, &'static str) {
        (self.key, self.owns_id_json)
    }

    fn value_predicate(self, operator: ComparisonOp, value: AttributeValue) -> Predicate<S> {
        Predicate::new(PredicateExpr::FieldValue {
            binding: self.key,
            owns_id_json: self.owns_id_json,
            operator,
            value,
        })
    }

    /// Equality against a canonical literal of the field's scalar domain.
    #[must_use]
    pub fn eq<O>(self, operand: O) -> Predicate<S>
    where
        V: QueryValued,
        O: QueryOperand<Domain = V::Domain>,
    {
        self.value_predicate(ComparisonOp::Equal, operand.into_operand())
    }

    /// Inequality against a canonical literal of the field's scalar domain.
    #[must_use]
    pub fn ne<O>(self, operand: O) -> Predicate<S>
    where
        V: QueryValued,
        O: QueryOperand<Domain = V::Domain>,
    {
        self.value_predicate(ComparisonOp::NotEqual, operand.into_operand())
    }

    /// Strictly-less ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn lt(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::LessThan, operand.into_operand())
    }

    /// Less-or-equal ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn le(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::LessThanOrEqual, operand.into_operand())
    }

    /// Strictly-greater ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn gt(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::GreaterThan, operand.into_operand())
    }

    /// Greater-or-equal ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn ge(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::GreaterThanOrEqual, operand.into_operand())
    }

    /// Text containment against bounded canonical text; admitted only for
    /// text field domains.
    #[must_use]
    pub fn contains(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::Contains,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Anchored text prefix against bounded canonical text; admitted only
    /// for text field domains.
    #[must_use]
    pub fn starts_with(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::StartsWith,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Anchored text suffix against bounded canonical text; admitted only
    /// for text field domains.
    #[must_use]
    pub fn ends_with(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::EndsWith,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Regular-expression match against a client-owned validated pattern;
    /// admitted only for text field domains.
    #[must_use]
    pub fn regex(self, pattern: crate::value::Regex) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::Regex,
            AttributeValue::String(pattern.into_string()),
        )
    }

    /// Compare against another compatible bound field of the same scalar
    /// value type; the comparison carries no literal.
    #[must_use]
    pub fn eq_field<Owner2>(self, other: BoundField<S, Owner2, V>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
    {
        Predicate::new(PredicateExpr::FieldField {
            left_binding: self.key,
            left_owns_id_json: self.owns_id_json,
            operator: ComparisonOp::Equal,
            right_binding: other.key,
            right_owns_id_json: other.owns_id_json,
        })
    }

    /// Order ascending by this bound field; missing keys fail closed unless
    /// an explicit missing-value policy is admitted.
    #[must_use]
    pub fn asc(self) -> Order<S> {
        Order {
            key: self.key,
            owns_id_json: self.owns_id_json,
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
            marker: PhantomData,
        }
    }

    /// Order descending by this bound field; missing keys fail closed unless
    /// an explicit missing-value policy is admitted.
    #[must_use]
    pub fn desc(self) -> Order<S> {
        Order {
            key: self.key,
            owns_id_json: self.owns_id_json,
            direction: SortDirection::Descending,
            missing: MissingOrder::Reject,
            marker: PhantomData,
        }
    }
}

/// One stable public ordering term over a bound field.
#[derive(Debug)]
pub struct Order<S: Schema> {
    key: BindingKey,
    owns_id_json: &'static str,
    direction: SortDirection,
    missing: MissingOrder,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Copy for Order<S> {}
impl<S: Schema> Clone for Order<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema> Order<S> {
    /// Admit missing keys and place them before all present keys.
    #[must_use]
    pub fn missing_first(mut self) -> Self {
        self.missing = MissingOrder::First;
        self
    }

    /// Admit missing keys and place them after all present keys.
    #[must_use]
    pub fn missing_last(mut self) -> Self {
        self.missing = MissingOrder::Last;
        self
    }
}

/// One typed collection selection used inside a distinct-root page shape.
pub struct Collected<S: Schema, B: Selectable<S>> {
    selection: B,
    distinct: bool,
    order: Vec<Order<S>>,
}

impl<S: Schema, B: Selectable<S>> Clone for Collected<S, B> {
    fn clone(&self) -> Self {
        Self {
            selection: self.selection,
            distinct: self.distinct,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema, B: Selectable<S>> Collected<S, B> {
    /// Deduplicate collection members by TypeDB concept identity.
    #[must_use]
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Append one stable order term owned by this collected binding.
    pub fn order_by(mut self, order: Order<S>) -> Result<Self> {
        if order.key != self.selection.binding_key() {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "collection_order_binding_mismatch",
                vec![],
                "collection ordering must reference the collected binding",
                None,
            ));
        }
        self.order.push(order);
        Ok(self)
    }
}

/// One generated relation role resolved against one relation binding
/// occurrence.
pub struct BoundRole<S: Schema, Owner: Model<Schema = S>, Players> {
    key: BindingKey,
    role_id_json: &'static str,
    marker: PhantomData<fn() -> (Owner, Players)>,
}

impl<S: Schema, Owner: Model<Schema = S>, Players> Copy for BoundRole<S, Owner, Players> {}
impl<S: Schema, Owner: Model<Schema = S>, Players> Clone for BoundRole<S, Owner, Players> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema, Owner: Model<Schema = S>, Players> BoundRole<S, Owner, Players> {
    /// Require this relation role to connect an admitted generated player
    /// binding.
    #[must_use]
    pub fn connects<MP: ThingModel<Schema = S>, ModeP: SelectionMode>(
        self,
        player: Binding<S, MP, ModeP>,
    ) -> Predicate<S>
    where
        Players: RolePlayer<MP>,
    {
        Predicate::new(PredicateExpr::Connects {
            relation: self.key,
            role_id_json: self.role_id_json,
            player: player.key,
        })
    }
}

mod selectable_sealed {
    pub trait Sealed {}
}

/// Sealed resolution from one selected binding to its typed query output.
pub trait Selectable<S: Schema>: selectable_sealed::Sealed + Copy {
    /// The materialized output type for one selected row.
    type Output;
    #[doc(hidden)]
    fn binding_key(self) -> BindingKey;
    #[doc(hidden)]
    fn materialize_output(row: &HydratedRow) -> std::result::Result<Self::Output, ValidationError>;

    #[doc(hidden)]
    fn __selection_handle(self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        Ok(session.handle_by_key(self.binding_key())?.one())
    }

    #[doc(hidden)]
    fn __materialize_slot(
        self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        let SlotValue::One(thing) = slot else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a collection slot for a singular selection",
                None,
            ));
        };
        let row = session.client_row_for(thing)?;
        Self::materialize_output(&row)
            .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
    }
}

impl<S: Schema, M: ThingModel<Schema = S> + CompleteModel> selectable_sealed::Sealed
    for Binding<S, M, Exact>
{
}
impl<S: Schema, M: ThingModel<Schema = S> + CompleteModel> Selectable<S> for Binding<S, M, Exact> {
    type Output = M;
    fn binding_key(self) -> BindingKey {
        self.key()
    }
    fn materialize_output(row: &HydratedRow) -> std::result::Result<M, ValidationError> {
        M::materialize(row, &HydrationCapability::new())
    }
}

impl<S: Schema, M: ThingModel<Schema = S> + SubtypeRootModel> selectable_sealed::Sealed
    for Binding<S, M, Subtypes>
{
}
impl<S: Schema, M: ThingModel<Schema = S> + SubtypeRootModel> Selectable<S>
    for Binding<S, M, Subtypes>
{
    type Output = M::Subtypes;
    fn binding_key(self) -> BindingKey {
        self.key()
    }
    fn materialize_output(row: &HydratedRow) -> std::result::Result<M::Subtypes, ValidationError> {
        M::__tb_dispatch_subtype(row, &HydrationCapability::new())
    }
}

mod selected_slot_sealed {
    pub trait Sealed<S> {}
}

/// Sealed resolution from one singular or collected selection to its typed
/// slot output.
#[doc(hidden)]
pub trait SelectedSlot<S: Schema>: selected_slot_sealed::Sealed<S> + Clone {
    type Output;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle>;

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output>;
}

impl<S: Schema, B: Selectable<S>> selected_slot_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SelectedSlot<S> for B {
    type Output = B::Output;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        (*self).__selection_handle(session)
    }

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        (*self).__materialize_slot(session, slot)
    }
}

impl<S: Schema, B: Selectable<S>> selected_slot_sealed::Sealed<S> for Collected<S, B> {}
impl<S: Schema, B: Selectable<S>> SelectedSlot<S> for Collected<S, B> {
    type Output = Vec<B::Output>;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        let binding = session.handle_by_key(self.selection.binding_key())?;
        let mut selection = binding
            .collect()
            .distinct(self.distinct)
            .map_err(Error::from_orm)?;
        for order in &self.order {
            selection = selection
                .order_by(session.lower_order(order)?)
                .map_err(Error::from_orm)?;
        }
        Ok(selection)
    }

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        let SlotValue::Many(things) = slot else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a singular slot for a collection selection",
                None,
            ));
        };
        let mut outputs = Vec::with_capacity(things.len());
        for thing in things {
            let row = session.client_row_for(thing)?;
            outputs.push(
                B::materialize_output(&row).map_err(|error| {
                    map_validation_error(error, ModelValidationPhase::Hydration)
                })?,
            );
        }
        Ok(outputs)
    }
}

mod selected_shape_sealed {
    pub trait Sealed<S> {}
}

/// Sealed typed selected-output shape accepted by the one query facade.
///
/// Implementations are supplied for one binding, positional tuples through
/// the canonical sixteen-slot ceiling, and derive-backed named rows.
pub trait SelectedShape<S: Schema>: selected_shape_sealed::Sealed<S> + Clone {
    /// One fully materialized public row.
    type Output;

    #[doc(hidden)]
    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle>;

    #[doc(hidden)]
    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output>;
}

impl<S: Schema, B: Selectable<S>> selected_shape_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SelectedShape<S> for B {
    type Output = B::Output;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        session
            .session
            .positional([SelectedSlot::__selection_handle(self, session)?])
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        let [slot] = row.slots() else {
            return Err(selected_shape_arity_error(1, row.slots().len()));
        };
        SelectedSlot::__materialize_slot(self, session, slot)
    }
}

impl<S: Schema, B: Selectable<S>> selected_shape_sealed::Sealed<S> for Collected<S, B> {}
impl<S: Schema, B: Selectable<S>> SelectedShape<S> for Collected<S, B> {
    type Output = Vec<B::Output>;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        session
            .session
            .positional([SelectedSlot::__selection_handle(self, session)?])
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        let [slot] = row.slots() else {
            return Err(selected_shape_arity_error(1, row.slots().len()));
        };
        SelectedSlot::__materialize_slot(self, session, slot)
    }
}

mod singular_selected_shape_sealed {
    pub trait Sealed<S> {}
}

/// Sealed marker for a selected shape containing singular slots only.
pub trait SingularSelectedShape<S: Schema>:
    SelectedShape<S> + singular_selected_shape_sealed::Sealed<S>
{
}

impl<S: Schema, B: Selectable<S>> singular_selected_shape_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SingularSelectedShape<S> for B {}

#[doc(hidden)]
pub trait SelectedTuple<S: Schema>: Clone {
    const ARITY: usize;

    type Output;

    fn __selection_handles(&self, session: &QuerySession<'_, S>)
    -> Result<Vec<OrmSelectionHandle>>;

    fn __materialize_slots(
        &self,
        session: &QuerySession<'_, S>,
        slots: &[SlotValue],
    ) -> Result<Self::Output>;
}

#[doc(hidden)]
pub trait SingularSelectedTuple<S: Schema>: SelectedTuple<S> {}

macro_rules! selected_tuple {
    ($length:literal; $(($type:ident, $index:tt)),+ $(,)?) => {
        impl<S: Schema, $($type: SelectedSlot<S>),+> SelectedTuple<S> for ($($type,)+) {
            const ARITY: usize = $length;

            type Output = ($(<$type as SelectedSlot<S>>::Output,)+);

            fn __selection_handles(
                &self,
                session: &QuerySession<'_, S>,
            ) -> Result<Vec<OrmSelectionHandle>> {
                Ok(vec![$(SelectedSlot::__selection_handle(&self.$index, session)?),+])
            }

            fn __materialize_slots(
                &self,
                session: &QuerySession<'_, S>,
                slots: &[SlotValue],
            ) -> Result<Self::Output> {
                let slots: &[SlotValue; $length] = slots
                    .try_into()
                    .map_err(|_| selected_shape_arity_error($length, slots.len()))?;
                Ok(($(SelectedSlot::__materialize_slot(
                    &self.$index,
                    session,
                    &slots[$index],
                )?,)+))
            }
        }

        impl<S: Schema, $($type: SelectedSlot<S>),+> selected_shape_sealed::Sealed<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: SelectedSlot<S>),+> SelectedShape<S> for ($($type,)+) {
            type Output = <Self as SelectedTuple<S>>::Output;

            fn __shape_handle(
                &self,
                session: &QuerySession<'_, S>,
            ) -> Result<OrmShapeHandle> {
                session
                    .session
                    .positional(self.__selection_handles(session)?)
                    .map_err(Error::from_orm)
            }

            fn __materialize_row(
                &self,
                session: &QuerySession<'_, S>,
                row: &MatchRow,
            ) -> Result<Self::Output> {
                self.__materialize_slots(session, row.slots())
            }
        }

        impl<S: Schema, $($type: Selectable<S>),+> singular_selected_shape_sealed::Sealed<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: Selectable<S>),+> SingularSelectedShape<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: Selectable<S>),+> SingularSelectedTuple<S>
            for ($($type,)+)
        {
        }
    };
}

selected_tuple!(1; (A, 0));
selected_tuple!(2; (A, 0), (B, 1));
selected_tuple!(3; (A, 0), (B, 1), (C, 2));
selected_tuple!(4; (A, 0), (B, 1), (C, 2), (D, 3));
selected_tuple!(5; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
selected_tuple!(6; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
selected_tuple!(7; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
selected_tuple!(8; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
selected_tuple!(9; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8));
selected_tuple!(10; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9));
selected_tuple!(11; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10));
selected_tuple!(12; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11));
selected_tuple!(13; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12));
selected_tuple!(14; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13));
selected_tuple!(15; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13), (O, 14));
selected_tuple!(16; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13), (O, 14), (P, 15));

/// Construction contract generated by `#[derive(type_bridge::SelectedRow)]`.
#[doc(hidden)]
pub trait SelectedRowSpec<Outputs>: Sized {
    fn __from_selected_outputs(outputs: Outputs) -> Self;
}

/// A declaration-ordered named selected shape produced by `SelectedRow`.
pub struct NamedSelection<S: Schema, Row, Slots> {
    slots: Slots,
    names: &'static [&'static str],
    marker: PhantomData<fn() -> (S, Row)>,
}

impl<S: Schema, Row, Slots: Clone> Clone for NamedSelection<S, Row, Slots> {
    fn clone(&self) -> Self {
        Self {
            slots: self.slots.clone(),
            names: self.names,
            marker: PhantomData,
        }
    }
}

impl<S: Schema, Row, Slots: SelectedTuple<S>> NamedSelection<S, Row, Slots> {
    #[doc(hidden)]
    pub fn __new(slots: Slots, names: &'static [&'static str]) -> Result<Self> {
        if names.len() != Slots::ARITY {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_selected_shape",
                vec![],
                format!(
                    "named selected shape has {} names for {} slots",
                    names.len(),
                    Slots::ARITY
                ),
                None,
            ));
        }
        Ok(Self {
            slots,
            names,
            marker: PhantomData,
        })
    }
}

impl<S: Schema, Row, Slots> selected_shape_sealed::Sealed<S> for NamedSelection<S, Row, Slots> {}

impl<S, Row, Slots> SelectedShape<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
    type Output = Row;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        let selections = self.slots.__selection_handles(session)?;
        session
            .session
            .named(
                self.names
                    .iter()
                    .copied()
                    .zip(selections)
                    .map(|(name, selection)| (name.to_owned(), selection)),
            )
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        Ok(Row::__from_selected_outputs(
            self.slots.__materialize_slots(session, row.slots())?,
        ))
    }
}

impl<S, Row, Slots> singular_selected_shape_sealed::Sealed<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SingularSelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
}

impl<S, Row, Slots> SingularSelectedShape<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SingularSelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
}

fn selected_shape_arity_error(expected: usize, actual: usize) -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "wrong_result_shape",
        vec![],
        format!("selected row has {actual} slots; expected {expected}"),
        None,
    )
}

/// Bounded options for one ordered row fetch.
#[derive(Debug)]
pub struct RowsOptions<S: Schema> {
    limit: u64,
    offset: u64,
    order: Vec<Order<S>>,
}

impl<S: Schema> Clone for RowsOptions<S> {
    fn clone(&self) -> Self {
        Self {
            limit: self.limit,
            offset: self.offset,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema> RowsOptions<S> {
    /// Create resource-bounded row options with a nonzero limit.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            offset: 0,
            order: Vec::new(),
        }
    }

    /// Skip the first `offset` distinct rows.
    #[must_use]
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Append one stable public ordering term.
    #[must_use]
    pub fn order_by(mut self, order: Order<S>) -> Self {
        self.order.push(order);
        self
    }
}

/// Bounded options for one ordered distinct-root page.
#[derive(Debug)]
pub struct PageOptions<S: Schema> {
    limit: u64,
    offset: u64,
    include_total: bool,
    order: Vec<Order<S>>,
}

impl<S: Schema> Clone for PageOptions<S> {
    fn clone(&self) -> Self {
        Self {
            limit: self.limit,
            offset: self.offset,
            include_total: self.include_total,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema> PageOptions<S> {
    /// Create resource-bounded page options with a nonzero terminal limit.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            offset: 0,
            include_total: false,
            order: Vec::new(),
        }
    }

    /// Skip the first `offset` distinct roots.
    #[must_use]
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Request a same-snapshot total distinct-root count.
    #[must_use]
    pub fn include_total(mut self, include_total: bool) -> Self {
        self.include_total = include_total;
        self
    }

    /// Append one stable root-ordering term.
    #[must_use]
    pub fn order_by(mut self, order: Order<S>) -> Self {
        self.order.push(order);
        self
    }
}

/// One immutable owned distinct-root page.
#[derive(Clone, Debug)]
pub struct Page<T> {
    items: Vec<T>,
    offset: u64,
    limit: u64,
    total: Option<u64>,
}

impl<T> Page<T> {
    /// Borrow page items in stable root order.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Return the requested root offset.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Return the requested root limit.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Return the same-snapshot total when it was requested.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    /// Consume the page and return its owned items.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

/// One persistent, reusable singular-shape query lineage.
///
/// Each authoring method returns a new lineage and leaves its ancestor
/// usable.
pub struct Query<'s, 'db, S: Schema, Shape: SelectedShape<S>> {
    session: &'s QuerySession<'db, S>,
    selection: Shape,
    predicates: Vec<Predicate<S>>,
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Clone for Query<'s, 'db, S, Shape> {
    fn clone(&self) -> Self {
        Self {
            session: self.session,
            selection: self.selection.clone(),
            predicates: self.predicates.clone(),
        }
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    /// Begin one persistent query lineage from a singular selected shape.
    pub fn query<Shape: SelectedShape<S>>(
        &self,
        selection: Shape,
    ) -> Result<Query<'_, 'db, S, Shape>> {
        selection.__shape_handle(self)?;
        Ok(Query {
            session: self,
            selection,
            predicates: Vec::new(),
        })
    }
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Query<'s, 'db, S, Shape> {
    /// Attach one predicate; repeated calls form a conjunction in call order.
    pub fn where_(&self, predicate: Predicate<S>) -> Result<Self> {
        let mut next = self.clone();
        next.predicates.push(predicate);
        Ok(next)
    }

    /// Attach predicates as one implicit conjunction in source order.
    pub fn where_all(&self, predicates: impl IntoIterator<Item = Predicate<S>>) -> Result<Self> {
        let mut next = self.clone();
        next.predicates.extend(predicates);
        Ok(next)
    }

    fn lineage(&self) -> Result<OrmQueryHandle> {
        self.lineage_with_hidden(None)
    }

    fn lineage_with_hidden(&self, hidden: Option<&OrmBindingHandle>) -> Result<OrmQueryHandle> {
        let shape = self.selection.__shape_handle(self.session)?;
        let mut query = self.session.session.query(shape).map_err(Error::from_orm)?;
        if let Some(hidden) = hidden {
            query = query.add_hidden(hidden.clone()).map_err(Error::from_orm)?;
        }
        for predicate in &self.predicates {
            let lowered = self.session.lower_predicate(&predicate.expr)?;
            query = query.where_predicate(lowered).map_err(Error::from_orm)?;
        }
        Ok(query)
    }

    pub(crate) fn validated_rows(
        &self,
        order: &[Order<S>],
        window: Window,
    ) -> Result<ValidatedMatchRequest> {
        let lineage = self.lineage()?;
        let mut lowered_orders = Vec::with_capacity(order.len());
        for term in order {
            lowered_orders.push(self.session.lower_order(term)?);
        }
        lineage
            .validate_fetch_rows(&lowered_orders, window, RowCardinality::BoundedMany)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_page<R: Selectable<S>>(
        &self,
        root: R,
        order: &[Order<S>],
        window: Window,
        include_total: bool,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        let lineage = self.lineage()?;
        let mut lowered_orders = Vec::with_capacity(order.len());
        for term in order {
            lowered_orders.push(self.session.lower_order(term)?);
        }
        lineage
            .validate_page_by(root, &lowered_orders, window, include_total)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_count_by<R: Selectable<S>>(
        &self,
        root: R,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        self.lineage()?
            .validate_count_by(root)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_exists_by<R: Selectable<S>>(
        &self,
        root: R,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        self.lineage()?
            .validate_exists_by(root)
            .map_err(Error::from_orm)
    }

    fn materialize_rows(&self, rows: &[MatchRow]) -> Result<Vec<Shape::Output>> {
        let mut outputs = Vec::with_capacity(rows.len());
        for row in rows {
            outputs.push(self.selection.__materialize_row(self.session, row)?);
        }
        Ok(outputs)
    }

    pub(crate) fn outputs_from_rows(
        &self,
        validated: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
    ) -> Result<Vec<Shape::Output>> {
        let rows = match result
            .for_request(validated)
            .map_err(|error| Error::from_orm(error.into()))?
        {
            MatchResult::Rows { rows } => rows,
            _ => {
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "provider returned a non-row result for a row fetch",
                    None,
                ));
            }
        };
        self.materialize_rows(rows)
    }

    pub(crate) fn output_page(
        &self,
        validated: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
    ) -> Result<Page<Shape::Output>> {
        let (entries, window, total) = match result
            .for_request(validated)
            .map_err(|error| Error::from_orm(error.into()))?
        {
            MatchResult::Page {
                entries,
                window,
                total,
                ..
            } => (entries, *window, *total),
            _ => {
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "provider returned a non-page result for a page fetch",
                    None,
                ));
            }
        };
        Ok(Page {
            items: self.materialize_rows(entries)?,
            offset: window.offset,
            limit: window.limit,
            total,
        })
    }

    async fn execute(
        &self,
        validated: ValidatedMatchRequest,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult)> {
        let result = match &self.session.execution {
            QueryExecution::Borrowed(transaction) => {
                transaction
                    .execute_match(&self.session.registry, &validated)
                    .await
            }
            QueryExecution::Local(database) => {
                database
                    .inner_orm()
                    .execute_match(&self.session.registry, &validated)
                    .await
            }
            QueryExecution::Remote(remote) => {
                return remote
                    .execute_match(&self.session.registry, validated)
                    .await;
            }
        };
        Ok((validated, result.map_err(Error::from_orm)?))
    }
}

impl<'s, 'db, S, Shape> Query<'s, 'db, S, Shape>
where
    S: Schema,
    Shape: SingularSelectedShape<S>,
{
    /// Return exactly one distinct selected identity, failing `no_result` on
    /// an empty stream and `not_unique` on more than one.
    pub async fn one(&self) -> Result<Shape::Output> {
        let validated = self.validated_rows(
            &[],
            Window {
                offset: 0,
                limit: 2,
            },
        )?;
        let (validated, result) = self.execute(validated).await?;
        let mut outputs = self.outputs_from_rows(&validated, &result)?;
        match outputs.len() {
            0 => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "no_result",
                vec![],
                "query selected no distinct identity",
                None,
            )),
            1 => Ok(outputs.remove(0)),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "not_unique",
                vec![],
                "query selected more than one distinct identity",
                None,
            )),
        }
    }

    /// Return a resource-bounded ordered sequence of distinct selected
    /// identities; the limit must be nonzero.
    pub async fn rows(&self, options: RowsOptions<S>) -> Result<Vec<Shape::Output>> {
        if options.limit == 0 {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "zero_limit",
                vec![],
                "row fetches require a nonzero limit",
                None,
            ));
        }
        let validated = self.validated_rows(
            &options.order,
            Window {
                offset: options.offset,
                limit: options.limit,
            },
        )?;
        let (validated, result) = self.execute(validated).await?;
        self.outputs_from_rows(&validated, &result)
    }

    /// Return the first distinct selected identity under a stable order.
    pub async fn first(&self, order: Order<S>) -> Result<Option<Shape::Output>> {
        let validated = self.validated_rows(
            &[order],
            Window {
                offset: 0,
                limit: 1,
            },
        )?;
        let (validated, result) = self.execute(validated).await?;
        Ok(self.outputs_from_rows(&validated, &result)?.pop())
    }
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Query<'s, 'db, S, Shape> {
    /// Return one resource-bounded page grouped by distinct root identity.
    pub async fn page_by<R: Selectable<S>>(
        &self,
        root: R,
        options: PageOptions<S>,
    ) -> Result<Page<Shape::Output>> {
        if options.limit == 0 {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "zero_limit",
                vec![],
                "page fetches require a nonzero limit",
                None,
            ));
        }
        let validated = self.validated_page(
            root,
            &options.order,
            Window {
                offset: options.offset,
                limit: options.limit,
            },
            options.include_total,
        )?;
        let (validated, result) = self.execute(validated).await?;
        self.output_page(&validated, &result)
    }

    /// Count distinct identities of one selected root binding.
    pub async fn count_by<R: Selectable<S>>(&self, root: R) -> Result<u64> {
        let validated = self.validated_count_by(root)?;
        let (validated, result) = self.execute(validated).await?;
        match result
            .for_request(&validated)
            .map_err(|error| Error::from_orm(error.into()))?
        {
            MatchResult::Count { value, .. } => Ok(*value),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-count result for a count",
                None,
            )),
        }
    }

    /// Test whether any distinct identity of one selected root binding
    /// exists.
    pub async fn exists_by<R: Selectable<S>>(&self, root: R) -> Result<bool> {
        let validated = self.validated_exists_by(root)?;
        let (validated, result) = self.execute(validated).await?;
        match result
            .for_request(&validated)
            .map_err(|error| Error::from_orm(error.into()))?
        {
            MatchResult::Exists { value, .. } => Ok(*value),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-existence result for an existence test",
                None,
            )),
        }
    }
}

impl<'s, 'db, S: Schema, B: Selectable<S>> Query<'s, 'db, S, B> {
    /// Count distinct selected identities.
    pub async fn count(&self) -> Result<u64> {
        self.count_by(self.selection).await
    }

    /// Test whether any distinct selected identity exists.
    pub async fn exists(&self) -> Result<bool> {
        self.exists_by(self.selection).await
    }

    fn lowered_reduce_terms(
        &self,
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<Vec<OrmFieldHandle>> {
        let mut lowered = Vec::new();
        for (_, input) in terms {
            if let Some((key, owns_id_json)) = input {
                lowered.push(self.session.lower_field(*key, owns_id_json)?);
            }
        }
        Ok(lowered)
    }

    pub(crate) fn validated_reduce(
        &self,
        group: Option<BindingKey>,
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(self.selection.binding_key())?;
        let group_handle = group
            .map(|key| self.session.handle_by_key(key))
            .transpose()?;
        let lineage = self.lineage_with_hidden(group_handle)?;
        let lowered_inputs = self.lowered_reduce_terms(terms)?;
        let mut inputs = lowered_inputs.iter();
        let mut pairs = Vec::with_capacity(terms.len());
        for (reduction, input) in terms {
            let handle = if input.is_some() {
                Some(inputs.next().expect("one lowered handle per input"))
            } else {
                None
            };
            pairs.push((*reduction, handle));
        }
        lineage
            .validate_reduce_by(root, group_handle, &pairs)
            .map_err(Error::from_orm)
    }

    fn decoded_reduction_rows(
        validated: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
    ) -> Result<Vec<type_bridge_orm::match_request::ReductionRow>> {
        match result
            .for_request(validated)
            .map_err(|error| Error::from_orm(error.into()))?
        {
            MatchResult::Reduction { rows, .. } => Ok(rows.clone()),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-reduction result for an aggregate",
                None,
            )),
        }
    }

    /// Reduce the distinct selected stream to one typed tuple of aggregate
    /// values.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<T::Output> {
        let term_list = terms.terms();
        let validated = self.validated_reduce(None, &term_list)?;
        let (validated, result) = self.execute(validated).await?;
        let rows = Self::decoded_reduction_rows(&validated, &result)?;
        let [row] = rows.as_slice() else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "ungrouped aggregates require exactly one reduction row",
                None,
            ));
        };
        T::decode(row.values())
    }

    /// Group the distinct selected stream by another attached binding's
    /// distinct identities before aggregating.
    pub fn group_by<G: Selectable<S>>(&self, group: G) -> Result<GroupedQuery<'s, 'db, S, B, G>> {
        self.session.handle_by_key(group.binding_key())?;
        Ok(GroupedQuery {
            query: self.clone(),
            group,
        })
    }
}

/// One query lineage grouped by a second attached binding for aggregation.
pub struct GroupedQuery<'s, 'db, S: Schema, B: Selectable<S>, G: Selectable<S>> {
    query: Query<'s, 'db, S, B>,
    group: G,
}

impl<'s, 'db, S: Schema, B: Selectable<S>, G: Selectable<S>> GroupedQuery<'s, 'db, S, B, G> {
    /// Reduce each witnessed distinct group identity to one typed tuple,
    /// returning materialized group keys with their aggregate values.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<Vec<(G::Output, T::Output)>> {
        let term_list = terms.terms();
        let validated = self
            .query
            .validated_reduce(Some(self.group.binding_key()), &term_list)?;
        let (validated, result) = self.query.execute(validated).await?;
        let rows = Query::<S, B>::decoded_reduction_rows(&validated, &result)?;
        let mut outputs = Vec::with_capacity(rows.len());
        for row in &rows {
            let thing = row.group().ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "grouped aggregates require group evidence per row",
                    None,
                )
            })?;
            let client_row = self.query.session.client_row_for(thing)?;
            let key = G::materialize_output(&client_row).map_err(|error| {
                crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
            })?;
            outputs.push((key, T::decode(row.values())?));
        }
        Ok(outputs)
    }
}
