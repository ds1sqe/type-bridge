//! The orders a provider statement can express.

use std::collections::BTreeSet;

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::FieldId;
use super::model::{FetchShape, FetchSlot, MatchOperation, MissingOrder};
use super::validation::{StableOrderSpec, ValidatedMatchRequest};
use crate::_descriptor::{OwnedAttributeDescriptor, TypeDescriptorRef};
use crate::_registry::DescriptorRegistry;

/// Refuse an order the provider statement cannot express.
///
/// The provider windows by sorting, and a sort has no place for a root whose
/// order field is absent. So a window cannot be ordered by a nullable field,
/// and missing values cannot be placed first or last. Row, page-root, and
/// collection orders are checked in that sequence, so the first offending
/// term is the one reported.
pub(crate) fn check_provider_orders(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
) -> Result<(), MatchError> {
    match &validated.request().operation {
        MatchOperation::FetchRows { .. } => check_order(registry, validated.stable_order()),
        MatchOperation::PageBy { output, .. } => {
            check_order(registry, validated.stable_order())?;
            let collected: BTreeSet<_> = output_slots(output)
                .filter_map(|slot| match slot {
                    FetchSlot::Collect { binding, .. } => Some(*binding),
                    FetchSlot::One { .. } => None,
                })
                .collect();
            for binding in collected {
                if let Some(order) = validated.collection_order(binding) {
                    check_order(registry, order)?;
                }
            }
            Ok(())
        }
        MatchOperation::CountBy { .. }
        | MatchOperation::ExistsBy { .. }
        | MatchOperation::ReduceBy { .. }
        | MatchOperation::ReduceByField { .. }
        | MatchOperation::ReduceByFields { .. } => Ok(()),
    }
}

fn check_order(registry: &DescriptorRegistry, order: &StableOrderSpec) -> Result<(), MatchError> {
    for term in order.terms() {
        let order = term.order();
        let attribute = owned_attribute(registry, &order.field.field)?;
        let is_required_scalar = !attribute.is_optional
            && attribute
                .cardinality()
                .is_none_or(|(minimum, maximum)| minimum >= 1 && maximum == Some(1));
        if !is_required_scalar {
            return Err(MatchError::new(
                MatchErrorCategory::UnsupportedCapability,
                "nullable_order_field_unsupported",
                "the selected provider cannot window by a nullable order field without filtering missing roots",
            )
            .at(MatchErrorPathSegment::Field(order.field.field.clone())));
        }
        if order.missing != MissingOrder::Reject {
            return Err(MatchError::new(
                MatchErrorCategory::UnsupportedCapability,
                "missing_value_order_unsupported",
                "the selected provider cannot preserve first/last missing-value ordering",
            )
            .at(MatchErrorPathSegment::Operation));
        }
    }
    Ok(())
}

fn owned_attribute(
    registry: &DescriptorRegistry,
    field: &FieldId,
) -> Result<OwnedAttributeDescriptor, MatchError> {
    let unregistered = || {
        MatchError::new(
            MatchErrorCategory::InvalidPlan,
            "missing_lowered_field_descriptor",
            "validated field owner is no longer registered",
        )
        .at(MatchErrorPathSegment::Operation)
        .at(MatchErrorPathSegment::Field(field.clone()))
    };
    let type_name = registry
        .descriptor_type_name(&field.owner)
        .ok_or_else(unregistered)?;
    let descriptor = registry.get(&type_name).ok_or_else(unregistered)?;
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(&field.name).cloned(),
        TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(&field.name).cloned(),
    }
    .ok_or_else(|| {
        MatchError::new(
            MatchErrorCategory::InvalidPlan,
            "missing_lowered_attribute",
            "validated field is no longer owned by its descriptor",
        )
        .at(MatchErrorPathSegment::Operation)
        .at(MatchErrorPathSegment::Field(field.clone()))
    })
}

fn output_slots(output: &FetchShape) -> impl Iterator<Item = &FetchSlot> {
    let slots: Vec<_> = match output {
        FetchShape::Positional { slots } => slots.iter().collect(),
        FetchShape::Named { slots } => slots.iter().map(|named| &named.slot).collect(),
    };
    slots.into_iter()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::EntityDescriptor;
    use crate::_entity::Annotation;
    use crate::match_request::model::{RowCardinality, SortDirection, Window};
    use crate::match_request::{SessionHandle, ValidatedMatchRequest};

    fn attribute(
        name: &str,
        annotations: Vec<Annotation>,
        optional: bool,
    ) -> OwnedAttributeDescriptor {
        OwnedAttributeDescriptor {
            field_name: name.into(),
            attr_name: name.into(),
            value_type: ValueType::String,
            annotations,
            is_optional: optional,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        }
    }

    fn registry() -> Arc<DescriptorRegistry> {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    attribute("name", vec![Annotation::Key], false),
                    attribute("nickname", Vec::new(), true),
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        Arc::new(registry)
    }

    /// The first ten persons ordered by one field.
    fn ordered_rows(
        registry: &Arc<DescriptorRegistry>,
        field: &str,
        missing: MissingOrder,
    ) -> ValidatedMatchRequest {
        let session = SessionHandle::new(Arc::clone(registry));
        let person = session.exact("person").unwrap();
        let order = person
            .field(field)
            .unwrap()
            .order(SortDirection::Ascending, missing);
        session
            .query(session.positional([person.one()]).unwrap())
            .unwrap()
            .validate_fetch_rows(
                &[order],
                Window {
                    offset: 0,
                    limit: 10,
                },
                RowCardinality::BoundedMany,
            )
            .unwrap()
    }

    #[test]
    fn a_window_ordered_by_a_required_scalar_is_admitted() {
        let registry = registry();
        check_provider_orders(
            &registry,
            &ordered_rows(&registry, "name", MissingOrder::Reject),
        )
        .unwrap();
    }

    #[test]
    fn a_window_ordered_by_a_nullable_field_is_refused() {
        let registry = registry();
        let error = check_provider_orders(
            &registry,
            &ordered_rows(&registry, "nickname", MissingOrder::Reject),
        )
        .unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
        assert_eq!(error.code().as_str(), "nullable_order_field_unsupported");
        assert!(matches!(
            error.path().segments(),
            [MatchErrorPathSegment::Field(field)] if field.name == "nickname"
        ));
    }

    #[test]
    fn placing_missing_values_first_or_last_is_refused() {
        let registry = registry();
        for missing in [MissingOrder::First, MissingOrder::Last] {
            let error = check_provider_orders(&registry, &ordered_rows(&registry, "name", missing))
                .unwrap_err();
            assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
            assert_eq!(error.code().as_str(), "missing_value_order_unsupported");
        }
    }

    #[test]
    fn an_operation_without_a_window_has_no_order_to_check() {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let count = session
            .query(session.positional([person.one()]).unwrap())
            .unwrap()
            .validate_count_by(&person)
            .unwrap();
        check_provider_orders(&registry, &count).unwrap();
    }
}
