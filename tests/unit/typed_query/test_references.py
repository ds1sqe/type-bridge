"""Runtime coverage for the owner-aware native reference/session facade."""

from __future__ import annotations

import pytest
import type_bridge_core

from tests.unit.typed_query._support import (
    corpus_error,
    diagnostic_session,
    invoke_untyped,
    runtime_attribute,
)
from tests.utils.handwritten import (
    Boolean,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    StringFieldRef,
    TypeFlags,
)
from type_bridge.typed import (
    BoundField,
    BoundRole,
    BoundVar,
    Collected,
    Predicate,
    QueryOrder,
    RoleRef,
)


class TypedName(String):
    pass


class TypedAge(Integer):
    pass


class TypedActive(Boolean):
    pass


class TypedUnowned(String):
    pass


class TypedPerson(Entity):
    flags = TypeFlags(name="typed-ref-person")
    name: TypedName = Flag(Key)
    age: TypedAge | None = None
    active: TypedActive | None = None


class TypedCompany(Entity):
    flags = TypeFlags(name="typed-ref-company")
    name: TypedName = Flag(Key)


class TypedEmployment(Relation):
    flags = TypeFlags(name="typed-ref-employment")
    employee: Role[TypedPerson] = Role("employee", TypedPerson)
    employer: Role[TypedCompany] = Role("employer", TypedCompany)


class TypedParty(Entity):
    flags = TypeFlags(name="typed-ref-party")
    name: TypedName = Flag(Key)


class TypedEmployee(TypedParty):
    flags = TypeFlags(name="typed-ref-employee")


class TypedAssociation(Relation):
    flags = TypeFlags(name="typed-ref-association")
    participant: Role[TypedPerson] = Role("participant", TypedPerson)


class TypedSpecialAssociation(TypedAssociation):
    flags = TypeFlags(name="typed-ref-special-association")


class TypedCollaboration(Relation):
    flags = TypeFlags(name="typed-ref-collaboration")
    participant: Role[TypedPerson] = Role("participant", TypedPerson)


class TypedComposition(Relation):
    flags = TypeFlags(name="typed-ref-composition")
    parent: Role[TypedParty] = Role("src", TypedParty)
    child: Role[TypedParty] = Role("dst", TypedParty)


PERSON_NAME = TypedName
PERSON_AGE = TypedAge
PERSON_ACTIVE = TypedActive


def test_same_model_variables_remain_distinct_and_predicates_are_persistent() -> None:
    session = diagnostic_session()
    first = session.var(TypedPerson)
    second = session.var(TypedPerson)

    first_name = first.field(PERSON_NAME)
    second_name = second.field(PERSON_NAME)
    comparison = first_name.neq(second_name)
    explicit_comparison = first_name.eq_field(second_name)
    literal = first_name.eq(TypedName("Alice"))

    assert isinstance(first, BoundVar)
    assert isinstance(comparison, Predicate)
    assert isinstance(explicit_comparison, Predicate)
    assert isinstance(comparison & literal, Predicate)
    assert isinstance(comparison | literal, Predicate)
    assert isinstance(~comparison, Predicate)
    assert comparison is not comparison.and_(literal)


def test_reference_categories_control_bound_operator_surface() -> None:
    person = diagnostic_session().var(TypedPerson)

    name = person.field(PERSON_NAME)
    age = person.field(PERSON_AGE)
    active = person.field(PERSON_ACTIVE)

    assert isinstance(name.contains(TypedName("lic")), Predicate)
    assert isinstance(name.asc(), QueryOrder)
    assert isinstance(age.gt(TypedAge(17)), Predicate)
    assert isinstance(age.desc(missing="last"), QueryOrder)
    assert isinstance(active.eq(TypedActive(True)), Predicate)
    assert not hasattr(active, "lt")
    assert not hasattr(active, "asc")


def test_attribute_class_tokens_are_resolved_through_the_bound_owner() -> None:
    person = diagnostic_session().var(TypedPerson)

    assert isinstance(person.field(TypedName).contains(TypedName("lic")), Predicate)
    with pytest.raises(TypeError, match="does not own attribute TypedUnowned"):
        person.field(TypedUnowned)


def test_relation_variable_registers_player_closure_and_connects_roles() -> None:
    session = diagnostic_session()
    employment = session.var(TypedEmployment)
    person = session.var(TypedPerson)
    company = session.var(TypedCompany)

    employee = employment.role(TypedEmployment.employee).is_(person)
    employer = employment.role(TypedEmployment.employer).connects(company)
    assert isinstance(employee & employer, Predicate)

    incompatible = invoke_untyped(
        employment.role(TypedEmployment.employee).connects,
        company,
    )
    assert isinstance(incompatible, Predicate)
    with pytest.raises(type_bridge_core.MatchRequestError) as raised:
        session.query(employment, company).where(incompatible).rows(limit=1)
    assert (raised.value.category, raised.value.code) == corpus_error(
        "references.incompatible-role-player"
    )


def test_bounded_reachability_is_session_owned_and_validates_before_execution() -> None:
    session = diagnostic_session()
    source = session.var(TypedEmployee)
    target = session.var(TypedParty)

    predicate = session.reachable(
        source,
        target,
        TypedComposition,
        TypedComposition.parent,
        TypedComposition.child,
        min_depth=0,
        max_depth=3,
    )
    assert isinstance(predicate, Predicate)
    assert session.query(target).match(source).where(predicate) is not None

    with pytest.raises(type_bridge_core.MatchRequestError) as reversed_bounds:
        session.reachable(
            source,
            target,
            TypedComposition,
            TypedComposition.parent,
            TypedComposition.child,
            min_depth=2,
            max_depth=1,
        )
    assert reversed_bounds.value.code == "reachable_bounds"

    with pytest.raises(type_bridge_core.MatchRequestError) as depth_limit:
        session.reachable(
            source,
            target,
            TypedComposition,
            TypedComposition.parent,
            TypedComposition.child,
            min_depth=0,
            max_depth=65,
        )
    assert depth_limit.value.code == "reachable_depth_limit"


@pytest.mark.parametrize(
    ("name", "value", "error"),
    [
        ("min_depth", True, TypeError),
        ("max_depth", 1.5, TypeError),
        ("min_depth", -1, ValueError),
        ("max_depth", 256, ValueError),
    ],
)
def test_bounded_reachability_rejects_noncanonical_depths(
    name: str,
    value: object,
    error: type[Exception],
) -> None:
    session = diagnostic_session()
    source = session.var(TypedParty)
    target = session.var(TypedParty)
    arguments: dict[str, object] = {"min_depth": 0, "max_depth": 1}
    arguments[name] = value

    with pytest.raises(error):
        invoke_untyped(
            session.reachable,
            source,
            target,
            TypedComposition,
            TypedComposition.parent,
            TypedComposition.child,
            **arguments,
        )


def test_bounded_reachability_rejects_forged_roles_and_cross_session_endpoints() -> None:
    session = diagnostic_session()
    source = session.var(TypedParty)
    target = session.var(TypedParty)

    with pytest.raises(TypeError, match="role_from does not belong"):
        invoke_untyped(
            session.reachable,
            source,
            target,
            TypedComposition,
            TypedAssociation.participant,
            TypedComposition.child,
            min_depth=1,
            max_depth=1,
        )

    foreign = diagnostic_session().var(TypedParty)
    with pytest.raises(type_bridge_core.MatchRequestError) as cross_session:
        session.reachable(
            source,
            foreign,
            TypedComposition,
            TypedComposition.parent,
            TypedComposition.child,
            min_depth=1,
            max_depth=1,
        )
    assert cross_session.value.code == "cross_session_handle"


def test_reference_owner_identity_is_enforced_by_native_handles() -> None:
    session = diagnostic_session()
    person = session.var(TypedPerson)
    session.var(TypedCompany)
    employment = session.var(TypedEmployment)
    session.var(TypedCollaboration)

    with pytest.raises(type_bridge_core.MatchRequestError) as field_error:
        invoke_untyped(person.field, runtime_attribute(TypedCompany, "name"))
    assert (field_error.value.category, field_error.value.code) == corpus_error(
        "references.cross-owner-field"
    )

    with pytest.raises(type_bridge_core.MatchRequestError) as role_error:
        invoke_untyped(
            employment.role,
            runtime_attribute(TypedCollaboration, "participant"),
        )
    assert (role_error.value.category, role_error.value.code) == corpus_error(
        "references.cross-owner-role"
    )


def test_parent_owned_references_bind_plain_inherited_members() -> None:
    session = diagnostic_session()
    employee = session.var(TypedEmployee)
    special = session.var(TypedSpecialAssociation)
    person = session.var(TypedPerson)

    inherited_field = invoke_untyped(
        employee.field,
        runtime_attribute(TypedParty, "name"),
    )
    inherited_role = invoke_untyped(
        special.role,
        runtime_attribute(TypedAssociation, "participant"),
    )
    assert isinstance(inherited_field, BoundField)
    assert isinstance(inherited_role, BoundRole)
    assert isinstance(inherited_field.eq(TypedName("Pat")), Predicate)
    assert isinstance(inherited_role.connects(person), Predicate)


def test_cross_session_references_fail_through_structured_native_errors() -> None:
    left = diagnostic_session().var(TypedPerson)
    right = diagnostic_session().var(TypedPerson)

    with pytest.raises(type_bridge_core.MatchRequestError) as raised:
        left.field(PERSON_NAME).eq(right.field(PERSON_NAME))

    assert raised.value.category == "invalid_plan"
    assert raised.value.code == "cross_session_handle"
    assert isinstance(raised.value.path, list)
    assert isinstance(raised.value.details, dict)

    employment = diagnostic_session().var(TypedEmployment)
    with pytest.raises(type_bridge_core.MatchRequestError) as role_error:
        employment.role(TypedEmployment.employee).connects(left)
    assert role_error.value.code == "cross_session_handle"


def test_public_reference_methods_reject_string_reconstruction() -> None:
    session = diagnostic_session()
    person = session.var(TypedPerson)
    employment = session.var(TypedEmployment)

    with pytest.raises(TypeError, match="owned Attribute class or owner-aware FieldRef"):
        invoke_untyped(person.field, "name")
    with pytest.raises(TypeError, match="owner-aware RoleRef"):
        invoke_untyped(employment.role, "employee")


def test_typed_builders_reject_legacy_constructor_reference_forgery() -> None:
    session = diagnostic_session()
    person = session.var(TypedPerson)
    employment = session.var(TypedEmployment)

    forged_field = StringFieldRef("name", TypedName, TypedPerson)
    forged_role = RoleRef(
        "employee",
        (TypedPerson,),
        owner_type=TypedEmployment,
    )

    with pytest.raises(TypeError, match="emitted by a model descriptor"):
        person.field(forged_field)
    with pytest.raises(TypeError, match="emitted by a model descriptor"):
        employment.role(forged_role)


def test_typed_builders_reject_mutated_descriptor_reference_identity() -> None:
    session = diagnostic_session()
    person = session.var(TypedPerson)
    employment = session.var(TypedEmployment)

    for attribute, replacement in [
        ("field_name", "age"),
        ("attr_type", TypedAge),
        ("entity_type", TypedCompany),
    ]:
        reference = runtime_attribute(TypedPerson, "name")
        setattr(reference, attribute, replacement)
        with pytest.raises(TypeError, match="emitted by a model descriptor"):
            invoke_untyped(person.field, reference)

    for attribute, replacement in [
        ("role_name", "employer"),
        ("player_types", (TypedCompany,)),
        ("owner_type", TypedCollaboration),
    ]:
        reference = runtime_attribute(TypedEmployment, "employee")
        setattr(reference, attribute, replacement)
        with pytest.raises(TypeError, match="emitted by a model descriptor"):
            invoke_untyped(employment.role, reference)


def test_typed_builders_reject_owner_type_names_mutated_after_reference_emission() -> None:
    session = diagnostic_session()
    company = session.var(TypedCompany)
    collaboration = session.var(TypedCollaboration)
    foreign_field = runtime_attribute(TypedPerson, "name")
    foreign_role = runtime_attribute(TypedAssociation, "participant")
    person_type_name = TypedPerson.flags.name
    association_type_name = TypedAssociation.flags.name

    try:
        TypedPerson.flags.name = TypedCompany.get_type_name()
        TypedAssociation.flags.name = TypedCollaboration.get_type_name()

        with pytest.raises(TypeError, match="emitted by a model descriptor"):
            invoke_untyped(company.field, foreign_field)
        with pytest.raises(TypeError, match="emitted by a model descriptor"):
            invoke_untyped(collaboration.role, foreign_role)
    finally:
        TypedPerson.flags.name = person_type_name
        TypedAssociation.flags.name = association_type_name


def test_typed_builders_reject_owner_aliases_emitted_after_mutation() -> None:
    session = diagnostic_session()
    company = session.var(TypedCompany)
    collaboration = session.var(TypedCollaboration)
    company_type_name = TypedCompany.flags.name
    collaboration_type_name = TypedCollaboration.flags.name
    person_type_name = TypedPerson.flags.name
    association_type_name = TypedAssociation.flags.name

    try:
        TypedPerson.flags.name = company_type_name
        TypedAssociation.flags.name = collaboration_type_name
        aliased_field = runtime_attribute(TypedPerson, "name")
        aliased_role = runtime_attribute(TypedAssociation, "participant")

        with pytest.raises(TypeError, match="reference owner does not match the bound model"):
            invoke_untyped(company.field, aliased_field)
        with pytest.raises(TypeError, match="reference owner does not match the bound model"):
            invoke_untyped(collaboration.role, aliased_role)

        TypedCompany.flags.name = "typed-ref-company-drifted"
        TypedCollaboration.flags.name = "typed-ref-collaboration-drifted"

        with pytest.raises(TypeError, match="bound variable model type name changed"):
            invoke_untyped(company.field, aliased_field)
        with pytest.raises(TypeError, match="bound variable model type name changed"):
            invoke_untyped(collaboration.role, aliased_role)
    finally:
        TypedCompany.flags.name = company_type_name
        TypedCollaboration.flags.name = collaboration_type_name
        TypedPerson.flags.name = person_type_name
        TypedAssociation.flags.name = association_type_name


def test_selection_transitions_are_opaque_and_persistent() -> None:
    person = diagnostic_session().subtypes(TypedPerson)
    base = person.collect()
    distinct = base.distinct()
    ordered = distinct.order_by(person.field(PERSON_NAME).asc())

    assert isinstance(base, Collected)
    assert isinstance(distinct, Collected)
    assert isinstance(ordered, Collected)
    assert base is not distinct
    assert distinct is not ordered
    for value in (person, base, ordered):
        assert not hasattr(value, "__dict__")
        assert not hasattr(value, "plan")
        assert not hasattr(value, "request_token")
