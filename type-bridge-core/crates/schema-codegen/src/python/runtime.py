from __future__ import annotations

import json
import logging
from collections.abc import Callable, Mapping, Sequence
from datetime import date, datetime, timedelta
from decimal import Decimal
from enum import Enum
from typing import (
    TYPE_CHECKING,
    Literal,
    Never,
    Protocol,
    Self,
    TypeGuard,
    cast,
    overload,
    runtime_checkable,
)

from type_bridge._runtime_projection import GeneratedEntityProjection, GeneratedRelationProjection

if TYPE_CHECKING:
    from type_bridge_core import PyProjectedModelManager, PyRuntimeProjection

    from type_bridge.session import Database, TransactionContext

    from ._query import QuerySession


logger = logging.getLogger(__name__)


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
    __runtime_projection__: PyRuntimeProjection
    __type_id__: str
    __model_form__: str
    _iid: str | None
    _values: dict[str, object]

    @property
    def iid(self) -> str | None:
        return self._iid

    @classmethod
    def manager(
        cls,
        connection: Database | TransactionContext,
    ) -> ProjectedModelManager[Self]:
        from type_bridge._runtime_projection import projected_manager_for

        projection = cls.__dict__.get("__runtime_projection__")
        if projection is None:
            raise RuntimeError("generated model package has no installed runtime projection")
        native = projected_manager_for(projection, cls, connection)
        return ProjectedModelManager(cls, native)

    @classmethod
    def query(cls, connection: Database | TransactionContext) -> QuerySession:
        from ._query import QuerySession

        return QuerySession(cls.__runtime_projection__, connection)

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


class CrudEvent(Enum):
    PRE_INSERT = "pre_insert"
    POST_INSERT = "post_insert"
    PRE_UPDATE = "pre_update"
    POST_UPDATE = "post_update"
    PRE_DELETE = "pre_delete"
    POST_DELETE = "post_delete"
    PRE_PUT = "pre_put"
    POST_PUT = "post_put"


class HookCancelled(Exception):  # noqa: N818 - intentional control-flow identity
    def __init__(
        self,
        reason: str = "",
        *,
        event: CrudEvent | None = None,
        hook: CrudHook[Never] | None = None,
    ) -> None:
        self.reason = reason
        self.event = event
        self.hook = hook
        super().__init__(reason)


class CrudHook[ModelT: ModelBase]:
    """Subclass and override only the generated-model lifecycle methods needed."""

    def should_run(self, event: CrudEvent, sender: type[ModelT]) -> bool:
        return True

    def pre_insert(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def post_insert(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def pre_update(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def post_update(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def pre_delete(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def post_delete(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def pre_put(self, sender: type[ModelT], instance: ModelT) -> None:
        pass

    def post_put(self, sender: type[ModelT], instance: ModelT) -> None:
        pass


class ProjectedModelNotFoundError(LookupError):
    """A strict generated-manager mutation could not resolve one input."""


class ProjectedModelManager[ModelT: ModelBase]:
    __slots__ = ("_filtered", "_hooks", "_model", "_native")

    def __init__(
        self,
        model: type[ModelT],
        native: PyProjectedModelManager,
        hooks: list[CrudHook[ModelT]] | None = None,
        *,
        filtered: bool = False,
    ) -> None:
        self._model = model
        self._native = native
        self._hooks = [] if hooks is None else hooks
        self._filtered = filtered

    def add_hook(self, hook: CrudHook[ModelT]) -> ProjectedModelManager[ModelT]:
        self._hooks.append(hook)
        return self

    def remove_hook(self, hook: CrudHook[ModelT]) -> None:
        self._hooks.remove(hook)

    def insert(self, instance: ModelT) -> ModelT:
        self._run_pre(CrudEvent.PRE_INSERT, instance)
        result = self._native.insert(instance)
        self._run_post(CrudEvent.POST_INSERT, result)
        return result

    def insert_many(self, instances: Sequence[ModelT]) -> list[ModelT]:
        values = list(instances)
        self._run_pre_many(CrudEvent.PRE_INSERT, values)
        result = self._native.insert_many(values)
        self._run_post_many(CrudEvent.POST_INSERT, result)
        return result

    def put(self, instance: ModelT) -> ModelT:
        self._run_pre(CrudEvent.PRE_PUT, instance)
        result = self._native.put(instance)
        self._run_post(CrudEvent.POST_PUT, result)
        return result

    def put_many(self, instances: Sequence[ModelT]) -> list[ModelT]:
        values = list(instances)
        self._run_pre_many(CrudEvent.PRE_PUT, values)
        result = self._native.put_many(values)
        self._run_post_many(CrudEvent.POST_PUT, result)
        return result

    def update(self, instance: ModelT) -> ModelT:
        self._run_pre(CrudEvent.PRE_UPDATE, instance)
        if self._resolve_instance_iid(instance):
            result = self._native.update(instance)
        else:
            result = instance
        self._run_post(CrudEvent.POST_UPDATE, result)
        return result

    def update_many(self, instances: Sequence[ModelT]) -> list[ModelT]:
        values = list(instances)
        self._run_pre_many(CrudEvent.PRE_UPDATE, values)
        resolved = [value for value in values if self._resolve_instance_iid(value)]
        self._native.update_many(resolved)
        self._run_post_many(CrudEvent.POST_UPDATE, values)
        return values

    @overload
    def delete(self, instance_or_iid: ModelT) -> ModelT: ...

    @overload
    def delete(self, instance_or_iid: str) -> None: ...

    @overload
    def delete(self) -> int: ...

    def delete(self, instance_or_iid: ModelT | str | None = None) -> ModelT | None | int:
        if instance_or_iid is None:
            if not self._filtered:
                raise TypeError("generated manager delete() without an instance requires filter()")
            instances = self.all()
            self._run_pre_many(CrudEvent.PRE_DELETE, instances)
            self._native.delete_many([cast(str, instance.iid) for instance in instances])
            self._run_post_many(CrudEvent.POST_DELETE, instances)
            return len(instances)
        if isinstance(instance_or_iid, str):
            self._native.delete(instance_or_iid)
            return None
        if not self._resolve_instance_iid(instance_or_iid):
            return instance_or_iid
        self._run_pre(CrudEvent.PRE_DELETE, instance_or_iid)
        self._native.delete(instance_or_iid)
        self._run_post(CrudEvent.POST_DELETE, instance_or_iid)
        return instance_or_iid

    def delete_many(
        self,
        instances: Sequence[ModelT],
        *,
        strict: bool = False,
    ) -> list[ModelT]:
        values = list(instances)
        resolved: list[ModelT] = []
        missing: list[ModelT] = []
        for value in values:
            if self._resolve_instance_iid(value):
                resolved.append(value)
            else:
                missing.append(value)
        if strict and missing:
            raise ProjectedModelNotFoundError("generated model(s) not found")
        self._run_pre_many(CrudEvent.PRE_DELETE, resolved)
        self._native.delete_many([cast(str, value.iid) for value in resolved])
        self._run_post_many(CrudEvent.POST_DELETE, resolved)
        return resolved

    def update_with(self, function: Callable[[ModelT], None]) -> list[ModelT]:
        if not self._filtered:
            raise TypeError("generated manager update_with() requires filter()")
        instances = self.all()
        for instance in instances:
            function(instance)
        return self.update_many(instances)

    def filter(self, **filters: object) -> ProjectedModelManager[ModelT]:
        return ProjectedModelManager(
            self._model,
            self._native.filter(**filters),
            self._hooks,
            filtered=True,
        )

    def all(self) -> list[ModelT]:
        return cast(list[ModelT], self._native.all())

    def first(self) -> ModelT | None:
        return cast(ModelT | None, self._native.first())

    def count(self) -> int:
        return self._native.count()

    def exists(self) -> bool:
        return self._native.exists()

    def get_by_iid(self, iid: str) -> ModelT | None:
        return cast(ModelT | None, self._native.get_by_iid(iid))

    def _resolve_instance_iid(self, instance: ModelT) -> bool:
        if instance.iid is not None:
            return True
        iid = self._native.resolve_iid(instance)
        if iid is None:
            return False
        instance.attach_runtime_iid(iid)
        return True

    def _run_pre(self, event: CrudEvent, instance: ModelT) -> None:
        for hook in self._hooks:
            if not hook.should_run(event, self._model):
                continue
            method = getattr(hook, event.value)
            try:
                method(self._model, instance)
            except HookCancelled as error:
                if error.event is None:
                    error.event = event
                if error.hook is None:
                    error.hook = hook
                raise

    def _run_post(self, event: CrudEvent, instance: ModelT) -> None:
        for hook in reversed(self._hooks):
            if not hook.should_run(event, self._model):
                continue
            method = getattr(hook, event.value)
            try:
                method(self._model, instance)
            except Exception:
                logger.exception("generated CRUD post-hook failed for %s", event.value)

    def _run_pre_many(self, event: CrudEvent, instances: Sequence[ModelT]) -> None:
        for instance in instances:
            self._run_pre(event, instance)

    def _run_post_many(self, event: CrudEvent, instances: Sequence[ModelT]) -> None:
        for instance in instances:
            self._run_post(event, instance)


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
        type(self).__runtime_projection__.validate_attribute_value(type(self), value)
        self.initialize_runtime_values({})
        object.__setattr__(self, "_attribute_value", value)

    @classmethod
    def _from_validated_query_value(cls, value: object) -> AttributeBase:
        cls.__runtime_projection__.validate_attribute_value(cls, value)
        instance = object.__new__(cls)
        instance.initialize_runtime_values({})
        object.__setattr__(instance, "_attribute_value", value)
        return instance

    @classmethod
    def owners(
        cls,
        connection: Database | TransactionContext,
        value: object = None,
        *,
        kind: Literal["entity", "relation"],
        lookup: str = "eq",
    ) -> list[ModelBase]:
        return _owner_lookup(
            connection,
            cls,
            value,
            kind=kind,
            lookup=lookup,
        )


class LongAttributeBase(AttributeBase):
    __slots__ = ()


class DoubleAttributeBase(AttributeBase):
    __slots__ = ()


class EntityBase(ModelBase, GeneratedEntityProjection):
    __slots__ = ()

    @classmethod
    def has(
        cls,
        connection: Database | TransactionContext,
        attribute: type[AttributeBase],
        value: object = None,
        *,
        lookup: str = "eq",
    ) -> list[ModelBase]:
        return _owner_lookup(
            connection,
            attribute,
            value,
            kind="entity",
            model=cls,
            lookup=lookup,
        )


class RelationBase(ModelBase, GeneratedRelationProjection):
    __slots__ = ()

    @classmethod
    def has(
        cls,
        connection: Database | TransactionContext,
        attribute: type[AttributeBase],
        value: object = None,
        *,
        lookup: str = "eq",
    ) -> list[ModelBase]:
        return _owner_lookup(
            connection,
            attribute,
            value,
            kind="relation",
            model=cls,
            lookup=lookup,
        )


_OWNER_LOOKUPS = frozenset(
    {
        "eq",
        "exact",
        "ne",
        "gt",
        "gte",
        "lt",
        "lte",
        "contains",
        "startswith",
        "endswith",
        "regex",
        "in",
        "present",
    }
)


def _is_model_class(value: object) -> TypeGuard[type[ModelBase]]:
    return isinstance(value, type) and issubclass(value, ModelBase)


def _is_attribute_class(value: object) -> TypeGuard[type[AttributeBase]]:
    return isinstance(value, type) and issubclass(value, AttributeBase)


def _model_is_abstract(model: type[ModelBase]) -> bool:
    declaration = _mapping(model.__projection__["declaration"])
    return _boolean(declaration["is_abstract"])


def _attribute_label(attribute: type[AttributeBase]) -> str:
    raw_identity: object = json.loads(attribute.__type_id__)
    identity = _mapping(raw_identity)
    if _string(identity["kind"]) != "attribute":
        raise TypeError("generated owner lookup requires an attribute model class")
    return _string(identity["label"])


def attribute_model_for_query_label(label: str) -> type[AttributeBase]:
    if _package_runtime_projection is None:
        raise RuntimeError("generated package runtime projection is not installed")
    for model in _package_models:
        if issubclass(model, AttributeBase) and _attribute_label(model) == label:
            return model
    raise TypeError("generated field identifies an unknown package attribute model")


def _field_name_for_attribute(
    model: type[ModelBase],
    attribute_label: str,
) -> str | None:
    query = _mapping(model.__projection__["query_tokens"])
    for raw_field in _sequence(query["fields"]):
        field = _mapping(raw_field)
        identity = _mapping(field["id"])
        if _string(identity["attribute"]) == attribute_label:
            return _string(field["target_name"])
    return None


def _owner_lookup(
    connection: Database | TransactionContext,
    attribute: object,
    value: object = None,
    *,
    kind: Literal["entity", "relation"],
    model: object | None = None,
    lookup: str = "eq",
) -> list[ModelBase]:
    """Find projected attribute owners and hydrate their exact generated models."""
    if not _is_attribute_class(attribute):
        raise TypeError("generated owner lookup requires an attribute model class")
    projection = _package_runtime_projection
    if projection is None:
        raise RuntimeError("generated package runtime projection is not installed")
    if attribute.__dict__.get("__runtime_projection__") is not projection:
        raise TypeError("generated owner lookup requires an attribute from this package")
    if lookup not in _OWNER_LOOKUPS:
        raise ValueError(f"unsupported generated owner lookup {lookup!r}")
    if lookup == "present" and value is not None:
        raise TypeError("generated owner lookup 'present' does not accept a value")
    if value is None and lookup not in {"eq", "exact", "present"}:
        raise TypeError("generated owner lookup comparison requires a value")

    expected_base: type[ModelBase]
    if kind == "entity":
        expected_base = EntityBase
    elif kind == "relation":
        expected_base = RelationBase
    else:
        raise ValueError("generated owner lookup kind must be 'entity' or 'relation'")

    if model is None:
        candidates = tuple(
            candidate
            for candidate in _package_models
            if issubclass(candidate, expected_base) and not _model_is_abstract(candidate)
        )
    else:
        if not _is_model_class(model) or not issubclass(model, expected_base):
            raise TypeError(f"generated {kind} owner lookup received the wrong model kind")
        if model.__dict__.get("__runtime_projection__") is not projection:
            raise TypeError("generated owner lookup requires a model from this package")
        candidates = tuple(
            candidate
            for candidate in _package_models
            if issubclass(candidate, model) and not _model_is_abstract(candidate)
        )

    attribute_label = _attribute_label(attribute)
    results: list[ModelBase] = []
    for candidate in candidates:
        field_name = _field_name_for_attribute(candidate, attribute_label)
        if field_name is None:
            continue
        filters: dict[str, object]
        if value is None:
            filters = {f"{field_name}__isnull": False}
        elif lookup == "in":
            if isinstance(value, (str, bytes)) or not _is_object_sequence(value):
                raise TypeError("generated owner lookup 'in' requires a non-string sequence")
            filters = {f"{field_name}__in": list(value)}
        else:
            filter_name = field_name if lookup in {"eq", "exact"} else f"{field_name}__{lookup}"
            filters = {filter_name: value}
        results.extend(candidate.manager(connection).filter(**filters).all())
    return results


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
        if not self.role and normalized is not None:
            values = normalized if _is_object_tuple(normalized) else (normalized,)
            for item in values:
                type(instance).__runtime_projection__.validate_field_value(
                    type(instance),
                    self.name,
                    item,
                )
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
    specifications = _sequence(create["players"]) if role else (create["value"],)
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
            return isinstance(value, _ProjectedStructValue) and value.__struct_id__ == _canonical(
                specification["value"]
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


_package_runtime_projection: PyRuntimeProjection | None = None
_package_models: tuple[type[ModelBase], ...] = ()


def install_runtime_projection(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[ModelBase], type[ReferenceBase] | None]],
) -> None:
    global _package_models, _package_runtime_projection
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
    _package_models = tuple(model for model, _reference in models)
    _package_runtime_projection = installed
    from ._query import install_projection

    install_projection(installed)
