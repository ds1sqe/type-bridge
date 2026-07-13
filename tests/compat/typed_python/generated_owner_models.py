"""Representative Python bindgen output for the built-wheel typing gate."""

from typing import TYPE_CHECKING

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags

if TYPE_CHECKING:
    from type_bridge.typed._descriptors import GeneratedStringFieldDescriptor


class GeneratedName(String):
    """Generated attribute used by the artifact consumer."""


class GeneratedParty(Entity):
    """Generated abstract owner of an inherited required key."""

    flags = TypeFlags(abstract=True)
    if TYPE_CHECKING:
        name: GeneratedStringFieldDescriptor["GeneratedParty", GeneratedName, GeneratedName]
    else:
        name: GeneratedName = Flag(Key)


class GeneratedPerson(GeneratedParty):
    """Generated concrete subtype used to prove owner rebinding."""

    flags = TypeFlags()


class GeneratedEmployment(Relation):
    """Generated relation used to prove owner-aware role inference."""

    flags = TypeFlags()
    employee: Role[GeneratedPerson] = Role("employee", GeneratedPerson)
