"""Regression tests for #179: Flag(...) defaults must not weaken constructors.

A ``Flag(...)`` default is a declaration-only sentinel. It must never make a
required field optional, and the ``AttributeFlags`` object must never leak as
a field value on a constructed instance.
"""

import pytest
from pydantic import ValidationError

from tests.utils.handwritten import (
    Card,
    Doc,
    Entity,
    Flag,
    Key,
    Meta,
    Ordered,
    Relation,
    Role,
    String,
    TypeFlags,
    Unique,
)


class PersonName(String):
    pass


class Nickname(String):
    pass


class TagLabel(String):
    pass


class TestRequiredFieldsStayRequired:
    """Required fields with Flag defaults must raise on no-arg construction."""

    def test_flag_unique_required_raises_on_missing(self):
        class UniquePerson(Entity):
            name: PersonName = Flag(Unique)

        with pytest.raises(ValidationError, match="name"):
            UniquePerson()

    def test_flag_key_required_raises_on_missing(self):
        class KeyedPerson(Entity):
            name: PersonName = Flag(Key)

        with pytest.raises(ValidationError, match="name"):
            KeyedPerson()

    def test_flag_doc_meta_required_raises_on_missing(self):
        class DocumentedPerson(Entity):
            name: PersonName = Flag(Doc("Full name."), Meta("owner", "hr"))

        with pytest.raises(ValidationError, match="name"):
            DocumentedPerson()

    def test_required_field_accepts_value(self):
        class ValuedPerson(Entity):
            name: PersonName = Flag(Unique)

        person = ValuedPerson(name=PersonName("alice"))
        assert isinstance(person.name, PersonName)
        assert person.name.value == "alice"


class TestOptionalAndListDefaults:
    """Optional and list fields keep their sentinel-free defaults."""

    def test_optional_field_with_flag_defaults_to_none(self):
        class OptionalPerson(Entity):
            name: PersonName | None = Flag(Unique)

        assert OptionalPerson().name is None

    def test_card_list_field_defaults_to_empty_list(self):
        class TaggedPerson(Entity):
            tags: list[TagLabel] = Flag(Card(0, 5))

        assert TaggedPerson().tags == []

    def test_ordered_list_field_defaults_to_empty_list(self):
        class OrderedPerson(Entity):
            tags: list[TagLabel] = Flag(Ordered)

        assert OrderedPerson().tags == []


class TestInheritedFields:
    """Inherited fields keep the parent's required/default semantics."""

    def test_inherited_required_field_raises_on_missing(self):
        class InheritBase(Entity):
            name: PersonName = Flag(Unique)

        class InheritChild(InheritBase):
            nickname: Nickname | None = None

        with pytest.raises(ValidationError, match="name"):
            InheritChild()

        child = InheritChild(name=PersonName("bob"))
        assert isinstance(child.name, PersonName)
        assert child.nickname is None

    def test_grandchild_required_field_raises_on_missing(self):
        class GrandBase(Entity):
            name: PersonName = Flag(Unique)

        class GrandMiddle(GrandBase):
            pass

        class GrandChild(GrandMiddle):
            pass

        with pytest.raises(ValidationError, match="name"):
            GrandChild()

    def test_python_base_parent_required_field_raises_on_missing(self):
        class PyOnlyBase(Entity):
            flags = TypeFlags(base=True)
            name: PersonName = Flag(Unique)

        class PyOnlyConcrete(PyOnlyBase):
            pass

        with pytest.raises(ValidationError, match="name"):
            PyOnlyConcrete()

        assert PyOnlyConcrete(name=PersonName("eve")).name.value == "eve"

    def test_inherited_real_default_is_preserved(self):
        class DefaultBase(Entity):
            name: PersonName = PersonName("default-name")

        class DefaultChild(DefaultBase):
            pass

        assert DefaultChild().name.value == "default-name"

    def test_inherited_list_and_optional_defaults_are_preserved(self):
        class MixedBase(Entity):
            tags: list[TagLabel] = Flag(Card(0, 9))
            nickname: Nickname | None = None

        class MixedChild(MixedBase):
            pass

        child = MixedChild()
        assert child.tags == []
        assert child.nickname is None


class TestRelationFields:
    """Relations share the same constructor semantics as entities."""

    def test_relation_required_field_raises_on_missing(self):
        class Employee(Entity):
            name: PersonName = Flag(Unique)

        class Employment(Relation):
            employee: Role = Role("employee", Employee)
            label: TagLabel = Flag(Doc("Employment label."))

        with pytest.raises(ValidationError, match="label"):
            Employment()

        employment = Employment(label=TagLabel("contract"))
        assert isinstance(employment.label, TagLabel)
