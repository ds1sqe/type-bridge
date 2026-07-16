from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from datetime import date, datetime, timedelta
from decimal import Decimal
from typing import Protocol, TypeGuard, runtime_checkable


def _is_object_dict(value: object) -> TypeGuard[dict[object, object]]:
    return isinstance(value, dict)


def _is_string_mapping(value: object) -> TypeGuard[dict[str, object]]:
    if not _is_object_dict(value):
        return False
    return all(isinstance(key, str) for key in value)


def _mapping(value: object) -> Mapping[str, object]:
    if not _is_string_mapping(value):
        raise TypeError("projected metadata value is not a string-keyed mapping")
    return value


def _is_object_list(value: object) -> TypeGuard[list[object]]:
    return isinstance(value, list)


def _is_object_sequence(value: object) -> TypeGuard[Sequence[object]]:
    return isinstance(value, Sequence)


def _is_object_tuple(value: object) -> TypeGuard[tuple[object, ...]]:
    return isinstance(value, tuple)


def _sequence(value: object) -> tuple[object, ...]:
    if not _is_object_list(value):
        raise TypeError("projected metadata value is not a list")
    return tuple(value)


def _string(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError("projected metadata value is not a string")
    return value


def _boolean(value: object) -> bool:
    if not isinstance(value, bool):
        raise TypeError("projected metadata value is not a boolean")
    return value


def _canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def load_mapping(source: str) -> Mapping[str, object]:
    value: object = json.loads(source)
    return _mapping(value)


class FieldToken:
    __slots__ = ("owner", "fact")

    def __init__(
        self,
        owner: type[ModelBase],
        fact: Mapping[str, object],
    ) -> None:
        self.owner = owner
        self.fact = fact


class RoleToken:
    __slots__ = ("owner", "fact")

    def __init__(
        self,
        owner: type[ModelBase],
        fact: Mapping[str, object],
    ) -> None:
        self.owner = owner
        self.fact = fact


class FunctionRef:
    __slots__ = ("id", "signature")

    def __init__(
        self,
        function_id: str,
        signature: Mapping[str, object],
    ) -> None:
        self.id = function_id
        self.signature = signature


class ModelBase:
    __slots__ = ("_iid", "_values")
    __projection__: Mapping[str, object]
    __runtime_projection__: object
    __type_id__: str
    __model_form__: str
    _iid: str | None
    _values: dict[str, object]

    @property
    def iid(self) -> str | None:
        return self._iid

    @classmethod
    def manager(cls, connection: object) -> object:
        from type_bridge._runtime_projection import projected_manager_for

        projection = cls.__dict__.get("__runtime_projection__")
        if projection is None:
            raise RuntimeError("generated model package has no installed runtime projection")
        return projected_manager_for(projection, cls, connection)

    def runtime_values(self) -> dict[str, object]:
        return self._values

    def initialize_runtime_values(
        self,
        values: Mapping[str, object],
    ) -> None:
        self._iid = None
        self._values = dict(values)

    def attach_runtime_iid(self, iid: str) -> None:
        if not iid:
            raise TypeError("projected IID must be a non-empty string")
        self._iid = iid


class AttributeBase(ModelBase):
    __slots__ = ("_attribute_value",)
    _attribute_value: object

    @property
    def value(self) -> object:
        return self._attribute_value

    def runtime_attribute_value(self) -> object:
        return self._attribute_value

    def initialize_runtime_attribute(self, value: object, scalar: str) -> None:
        if not _matches_scalar(value, scalar):
            raise TypeError("projected attribute has an incompatible scalar value")
        self.initialize_runtime_values({})
        object.__setattr__(self, "_attribute_value", value)


class EntityBase(ModelBase):
    __slots__ = ()


class RelationBase(ModelBase):
    __slots__ = ()


class ReferenceBase:
    __slots__ = ("_iid", "_values")
    __projection__: Mapping[str, object]
    __type_id__: str
    __model_form__: str
    _iid: str
    _values: dict[str, object]

    @property
    def iid(self) -> str:
        return self._iid

    def runtime_values(self) -> dict[str, object]:
        return self._values

    def initialize_runtime_reference(
        self,
        iid: str,
        values: Mapping[str, object],
    ) -> None:
        self._iid = iid
        self._values = dict(values)


class StructValueBase:
    __slots__ = ()
    __struct_id__: str

    def __setattr__(self, name: str, value: object) -> None:
        raise AttributeError("projected struct values are immutable")


@runtime_checkable
class _ProjectedModelValue(Protocol):
    __type_id__: str
    __model_form__: str


@runtime_checkable
class _ProjectedStructValue(Protocol):
    __struct_id__: str


class _Descriptor(Protocol):
    def __set_name__(self, owner: type[object], name: str) -> None: ...


class _ProjectedField:
    __slots__ = ("fact", "create", "read", "name", "role")

    def __init__(
        self,
        fact: Mapping[str, object],
        create: Mapping[str, object] | None,
        read: Mapping[str, object] | None,
        *,
        role: bool,
    ) -> None:
        self.fact = fact
        self.create = create
        self.read = read
        self.name = ""
        self.role = role

    def __set_name__(self, owner: type[object], name: str) -> None:
        self.name = name

    def __get__(
        self,
        instance: ModelBase | None,
        owner: type[ModelBase],
    ) -> object:
        if instance is None:
            if self.role:
                return RoleToken(owner, self.fact)
            return FieldToken(owner, self.fact)
        values = instance.runtime_values()
        if self.name not in values:
            raise AttributeError(self.name)
        return values[self.name]

    def __set__(self, instance: ModelBase, value: object) -> None:
        if self.create is None:
            raise AttributeError(f"{self.name} is read-only")
        multiplicity = _mapping(self.create["multiplicity"])
        normalized = _normalize(value, multiplicity)
        _validate_projected(normalized, self.create, role=self.role)
        instance.runtime_values()[self.name] = normalized


class _ReferenceField:
    __slots__ = ("name",)

    def __init__(self) -> None:
        self.name = ""

    def __set_name__(self, owner: type[object], name: str) -> None:
        self.name = name

    def __get__(
        self,
        instance: ReferenceBase | None,
        owner: type[ReferenceBase],
    ) -> object:
        if instance is None:
            return self
        return instance.runtime_values()[self.name]


def _normalize(
    value: object,
    multiplicity: Mapping[str, object],
) -> object:
    if _string(multiplicity["container"]) == "scalar":
        if value is None and _boolean(multiplicity["required"]):
            raise ValueError("required projected value is absent")
        return value
    if isinstance(value, (str, bytes)) or not _is_object_sequence(value):
        raise TypeError("projected multi-value field requires a sequence")
    values: tuple[object, ...] = tuple(value)
    cardinality = _mapping(multiplicity["cardinality"])
    minimum = int(_string(cardinality["min"]))
    maximum = _string(cardinality["max"])
    if len(values) < minimum:
        raise ValueError("projected value is below its cardinality minimum")
    if maximum != "unbounded" and len(values) > int(maximum):
        raise ValueError("projected value exceeds its cardinality maximum")
    return values


def _validate_projected(
    value: object,
    create: Mapping[str, object],
    *,
    role: bool,
) -> None:
    if value is None:
        return
    multiplicity = _mapping(create["multiplicity"])
    if _string(multiplicity["container"]) == "sequence":
        if not _is_object_tuple(value):
            raise TypeError("normalized projected sequence is not a tuple")
        values: tuple[object, ...] = value
    else:
        values = (value,)
    specifications = (
        _sequence(create["players"])
        if role
        else (create["value"],)
    )
    for item in values:
        if not any(
            _matches_projected(item, _mapping(specification), role=role)
            for specification in specifications
        ):
            raise TypeError("projected value has an incompatible runtime type")


def _matches_projected(
    value: object,
    specification: Mapping[str, object],
    *,
    role: bool,
) -> bool:
    if role:
        model_use = specification
    else:
        kind = _string(specification["kind"])
        if kind == "model":
            model_use = _mapping(specification["value"])
        elif kind == "struct":
            return (
                isinstance(value, _ProjectedStructValue)
                and value.__struct_id__ == _canonical(specification["value"])
            )
        else:
            return _matches_scalar(value, _string(specification["value"]))
    return (
        isinstance(value, _ProjectedModelValue)
        and value.__type_id__ == _canonical(model_use["id"])
        and value.__model_form__ == _string(model_use["form"])
    )


def _matches_scalar(value: object, scalar: str) -> bool:
    if scalar == "string":
        return isinstance(value, str)
    if scalar == "long":
        return type(value) is int
    if scalar == "double":
        return type(value) is float
    if scalar == "boolean":
        return type(value) is bool
    if scalar == "date":
        return type(value) is date
    if scalar == "datetime":
        return type(value) is datetime and value.tzinfo is None
    if scalar == "datetime_tz":
        return type(value) is datetime and value.tzinfo is not None
    if scalar == "decimal":
        return type(value) is Decimal
    if scalar == "duration":
        return type(value) is timedelta
    return False


def initialize_model(
    instance: ModelBase,
    values: Mapping[str, object],
) -> None:
    instance.initialize_runtime_values({})
    for name, value in values.items():
        setattr(instance, name, value)


def initialize_reference(
    instance: ReferenceBase,
    iid: str,
    values: Mapping[str, object],
) -> None:
    instance.initialize_runtime_reference(iid, values)


def initialize_attribute(
    instance: AttributeBase,
    value: object,
    scalar: str,
) -> None:
    instance.initialize_runtime_attribute(value, scalar)


def freeze_struct(
    instance: StructValueBase,
    values: Mapping[str, object],
) -> None:
    for name, value in values.items():
        object.__setattr__(instance, name, value)


def _by_identity(
    values: object,
    key: str,
) -> dict[str, Mapping[str, object]]:
    indexed: dict[str, Mapping[str, object]] = {}
    for value in _sequence(values):
        item = _mapping(value)
        indexed[_canonical(item[key])] = item
    return indexed


def _install(
    owner: type[object],
    name: str,
    descriptor: _Descriptor,
) -> None:
    setattr(owner, name, descriptor)
    descriptor.__set_name__(owner, name)


def install_model(
    owner: type[ModelBase],
    reference: type[ReferenceBase] | None,
    projection: Mapping[str, object],
) -> None:
    owner.__projection__ = projection
    create = _mapping(projection["create"])
    read = _mapping(projection["complete_read"])
    query = _mapping(projection["query_tokens"])
    create_fields = _by_identity(create["fields"], "token")
    read_fields = _by_identity(read["fields"], "token")
    create_roles = _by_identity(create["roles"], "role")
    read_roles = _by_identity(read["roles"], "role")
    query_fields = _by_identity(query["fields"], "id")
    for identity, fact in query_fields.items():
        _install(
            owner,
            _string(fact["target_name"]),
            _ProjectedField(
                fact,
                create_fields.get(identity),
                read_fields.get(identity),
                role=False,
            ),
        )
    for identity, fact in _by_identity(query["roles"], "role").items():
        _install(
            owner,
            _string(fact["target_name"]),
            _ProjectedField(
                fact,
                create_roles.get(identity),
                read_roles.get(identity),
                role=True,
            ),
        )
    if reference is not None:
        reference.__projection__ = _mapping(projection["reference_read"])
        for identity in _sequence(reference.__projection__["key_fields"]):
            fact = query_fields[_canonical(identity)]
            _install(reference, _string(fact["target_name"]), _ReferenceField())


_package_runtime_projection: object | None = None


def install_runtime_projection(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[ModelBase], type[ReferenceBase] | None]],
) -> None:
    global _package_runtime_projection
    if _package_runtime_projection is not None:
        raise RuntimeError("generated package runtime projection is already installed")
    from type_bridge._runtime_projection import install_runtime_projection as install_native

    installed = install_native(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
    )
    for model, _reference in models:
        model.__runtime_projection__ = installed
    _package_runtime_projection = installed
