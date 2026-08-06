//! Typed reduction operation: validation, capability, and recording proofs.

use std::sync::Arc;

use type_bridge_orm::match_request::{
    Capability, MatchError, MatchResult, RecordingMatchExecutor, RecordingMatchResponse,
    ReducedValue, Reduction, SessionHandle,
};
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attribute(field_name: &str, attr_name: &str, value_type: ValueType) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.to_owned(),
        attr_name: attr_name.to_owned(),
        value_type,
        annotations: Vec::new(),
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                OwnedAttributeDescriptor {
                    annotations: vec![Annotation::Key],
                    ..attribute("name", "person-name", ValueType::String)
                },
                attribute("age", "person-age", ValueType::Long),
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "department".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                annotations: vec![Annotation::Key],
                ..attribute("name", "department-name", ValueType::String)
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
}

fn code_of(error: &MatchError) -> &str {
    error.code().as_str()
}

#[test]
fn typed_reductions_validate_and_replay_scalar_recording_evidence() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let age = person.field("age").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap();

    let validated = query
        .validate_reduce_by(
            &person,
            None,
            &[
                (Reduction::Count, None),
                (Reduction::Mean, Some(&age)),
                (Reduction::Sum, Some(&age)),
            ],
        )
        .unwrap();
    assert!(
        validated
            .capabilities()
            .contains(Capability::TypedReduction)
    );

    let mut executor = RecordingMatchExecutor::new(Arc::clone(&registry));
    executor.push(RecordingMatchResponse::Reduction(vec![
        ReducedValue::Count(3),
        ReducedValue::Double(Some(41.5)),
        ReducedValue::Long(Some(124)),
    ]));
    let result = executor.execute(&validated).unwrap();
    let MatchResult::Reduction {
        root: _,
        group,
        rows,
    } = result.for_request(&validated).unwrap()
    else {
        panic!("expected a reduction result")
    };
    assert!(group.is_none());
    assert_eq!(rows.len(), 1);
    assert!(rows[0].group().is_none());
    assert_eq!(
        rows[0].values(),
        &[
            ReducedValue::Count(3),
            ReducedValue::Double(Some(41.5)),
            ReducedValue::Long(Some(124)),
        ]
    );

    // Absent values are admitted on the ungrouped empty stream.
    let validated = query
        .validate_reduce_by(&person, None, &[(Reduction::Median, Some(&age))])
        .unwrap();
    executor.push(RecordingMatchResponse::Reduction(vec![
        ReducedValue::Double(None),
    ]));
    let result = executor.execute(&validated).unwrap();
    let MatchResult::Reduction { rows, .. } = result.for_request(&validated).unwrap() else {
        panic!("expected a reduction result")
    };
    assert_eq!(rows[0].values(), &[ReducedValue::Double(None)]);
    assert_eq!(executor.calls(), 2);
}

#[test]
fn grouped_reductions_validate_and_admit_zero_witnessed_groups() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let department = session.exact("department").unwrap();
    let age = person.field("age").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap()
        .add_hidden(department.clone())
        .unwrap()
        .allow_cross_join(&person, &department)
        .unwrap();

    let validated = query
        .validate_reduce_by(
            &person,
            Some(&department),
            &[(Reduction::Count, None), (Reduction::Std, Some(&age))],
        )
        .unwrap();
    let mut executor = RecordingMatchExecutor::new(Arc::clone(&registry));
    executor.push(RecordingMatchResponse::EmptyGroupedReduction);
    let result = executor.execute(&validated).unwrap();
    let MatchResult::Reduction { group, rows, .. } = result.for_request(&validated).unwrap() else {
        panic!("expected a reduction result")
    };
    assert!(group.is_some());
    assert!(rows.is_empty());
}

#[test]
fn typed_reductions_reject_exact_validation_and_evidence_failures() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let age = person.field("age").unwrap();
    let name = person.field("name").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap();

    // The group binding must differ from the reduced root.
    let error = query
        .validate_reduce_by(&person, Some(&person), &[(Reduction::Count, None)])
        .unwrap_err();
    assert!(error.to_string().contains("reduce_group_is_root"));

    // At least one reducer term is required.
    let error = query.validate_reduce_by(&person, None, &[]).unwrap_err();
    assert!(error.to_string().contains("empty_reducers"));

    // The reducer list is bounded by the selection ceiling.
    let terms: Vec<_> = (0..17).map(|_| (Reduction::Count, None)).collect();
    let error = query.validate_reduce_by(&person, None, &terms).unwrap_err();
    assert!(error.to_string().contains("reducer_cap_exceeded"));

    // Input-requiring reducers demand a bound scalar field.
    let error = query
        .validate_reduce_by(&person, None, &[(Reduction::Sum, None)])
        .unwrap_err();
    assert!(error.to_string().contains("reduce_input_required"));

    // Numeric reducers reject non-numeric scalar inputs.
    let error = query
        .validate_reduce_by(&person, None, &[(Reduction::Mean, Some(&name))])
        .unwrap_err();
    assert!(error.to_string().contains("reduce_input_domain"));

    // Provider evidence with the wrong arity fails closed.
    let validated = query
        .validate_reduce_by(&person, None, &[(Reduction::Count, None)])
        .unwrap();
    let mut executor = RecordingMatchExecutor::new(Arc::clone(&registry));
    executor.push(RecordingMatchResponse::Reduction(vec![
        ReducedValue::Count(1),
        ReducedValue::Count(2),
    ]));
    let error = executor.execute(&validated).unwrap_err();
    assert_eq!(code_of(&error), "reduction_arity_mismatch");

    // Provider evidence with the wrong value variant fails closed.
    let validated = query
        .validate_reduce_by(&person, None, &[(Reduction::Mean, Some(&age))])
        .unwrap();
    executor.push(RecordingMatchResponse::Reduction(vec![ReducedValue::Long(
        Some(41),
    )]));
    let error = executor.execute(&validated).unwrap_err();
    assert_eq!(code_of(&error), "reduction_value_domain");
}
