"""Native descriptor/session ownership for owner-aware typed variables."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import fields, is_dataclass
from types import MappingProxyType
from typing import Literal, Self, TypeVar, get_args, get_origin, get_type_hints, overload

from type_bridge_core import (
    MatchSessionHandle as _NativeSessionHandle,
)
from type_bridge_core import (
    PyDescriptorRegistry as _NativeDescriptorRegistry,
)

from type_bridge._rust_runtime import descriptor_for_model
from type_bridge.models.base import TypeDBType
from type_bridge.models.entity import Entity
from type_bridge.models.relation import Relation
from type_bridge.session import Database, TransactionContext
from type_bridge.typed.query import Query
from type_bridge.typed.references import BoundVar, Selection

ModelT = TypeVar("ModelT", bound=TypeDBType)
_FRAMEWORK_MODEL_ROOTS = (TypeDBType, Entity, Relation)


class QuerySession:
    """Own one native match lineage and its real connection context."""

    __slots__ = (
        "__registry",
        "__handle",
        "__connection",
        "__models",
        "__model_view",
    )

    def __init__(self, connection: Database | TransactionContext) -> None:
        if not isinstance(connection, (Database, TransactionContext)):
            raise TypeError("QuerySession requires a Database or TransactionContext")
        self.__initialize(connection)

    @classmethod
    def _diagnostic(cls) -> Self:
        """Create an internal construction-only session without an execution target."""
        session = cls.__new__(cls)
        session.__initialize(None)
        return session

    def __initialize(self, connection: Database | TransactionContext | None) -> None:
        registry = _NativeDescriptorRegistry()
        self.__registry = registry
        self.__handle = _NativeSessionHandle(registry)
        self.__connection = connection
        self.__models: dict[str, type[TypeDBType]] = {}
        self.__model_view = MappingProxyType(self.__models)

    @overload
    def var(self, model: type[ModelT], *, subtypes: Literal[False] = False) -> BoundVar[ModelT]: ...

    @overload
    def var(self, model: type[ModelT], *, subtypes: Literal[True]) -> BoundVar[ModelT]: ...

    def var(self, model: type[ModelT], *, subtypes: bool = False) -> BoundVar[ModelT]:
        """Create a fresh exact or subtype-inclusive native variable."""
        if not isinstance(model, type) or not issubclass(model, TypeDBType):
            raise TypeError("QuerySession.var requires an Entity or Relation model class")
        if model in _FRAMEWORK_MODEL_ROOTS:
            raise TypeError("QuerySession.var requires a declared Entity or Relation model class")
        registered: set[type[TypeDBType]] = set()
        subtype_roots: set[type[TypeDBType]] = set()
        self._register_descriptor_closure(model, registered, subtype_roots)
        if subtypes:
            self._register_loaded_subtypes(model, registered, subtype_roots)
        type_name = model.get_type_name()
        handle = self.__handle.subtypes(type_name) if subtypes else self.__handle.exact(type_name)
        return BoundVar._from_native(handle, model, type_name)

    def exact(self, model: type[ModelT]) -> BoundVar[ModelT]:
        """Create a fresh exact-match variable for ``model``."""
        return self.var(model)

    def subtypes(self, model: type[ModelT]) -> BoundVar[ModelT]:
        """Create a fresh subtype-inclusive variable for ``model``."""
        return self.var(model, subtypes=True)

    # BEGIN GENERATED QUERY OVERLOADS
    @overload
    def query[T1](
        self,
        selection1: Selection[T1],
        /,
    ) -> Query[T1]: ...

    @overload
    def query[T1, T2](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        /,
    ) -> Query[T1, T2]: ...

    @overload
    def query[T1, T2, T3](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        /,
    ) -> Query[T1, T2, T3]: ...

    @overload
    def query[T1, T2, T3, T4](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        /,
    ) -> Query[T1, T2, T3, T4]: ...

    @overload
    def query[T1, T2, T3, T4, T5](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        /,
    ) -> Query[T1, T2, T3, T4, T5]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        selection16: Selection[T16],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16]: ...

    # END GENERATED QUERY OVERLOADS

    def query(self, *selections: Selection[object]) -> object:
        """Create one native positional query with between 1 and 16 slots."""
        if any(not isinstance(selection, Selection) for selection in selections):
            raise TypeError("QuerySession.query requires Selection values")
        shape = self.__handle.positional(
            [selection._native_selection() for selection in selections]
        )
        handle = self.__handle.query(shape)
        return Query._from_native(
            handle,
            self.__registry,
            self.__connection,
            self.__model_view,
        )

    def query_as[DeclaredRowT](
        self,
        declaration: type[DeclaredRowT],
        /,
        **selections: Selection[object],
    ) -> Query[DeclaredRowT]:
        """Create a native named shape checked against one immutable row type."""
        declarations = _named_declaration(declaration)
        for selection in selections.values():
            if not isinstance(selection, Selection):
                raise TypeError("QuerySession.query_as values must be Selection values")

        names = list(selections)
        selected = list(selections.values())
        shape = self.__handle.named_checked(
            declarations,
            names,
            [selection._native_selection() for selection in selected],
        )
        handle = self.__handle.query(shape)
        return Query._from_native(
            handle,
            self.__registry,
            self.__connection,
            self.__model_view,
            declaration,
        )

    def _native_session(self) -> _NativeSessionHandle:
        return self.__handle

    def _native_registry(self) -> _NativeDescriptorRegistry:
        return self.__registry

    def _model_constructors(self) -> Mapping[str, type[TypeDBType]]:
        return self.__model_view

    def _register_descriptor_closure(
        self,
        model: type[TypeDBType],
        seen: set[type[TypeDBType]],
        subtype_roots: set[type[TypeDBType]],
    ) -> None:
        if model in seen:
            return
        if model in _FRAMEWORK_MODEL_ROOTS:
            seen.add(model)
            return
        type_name = model.get_type_name()
        existing = self.__models.get(type_name)
        if existing is not None and existing is not model:
            raise TypeError(
                "QuerySession model constructor collision for TypeDB label "
                f"{type_name!r}: {existing.__name__} cannot be replaced by {model.__name__}"
            )
        seen.add(model)

        for base in reversed(model.__mro__[1:]):
            if (
                isinstance(base, type)
                and issubclass(base, TypeDBType)
                and base not in _FRAMEWORK_MODEL_ROOTS
                and not base.is_base()
            ):
                self._register_descriptor_closure(base, seen, subtype_roots)

        if issubclass(model, Relation):
            for relation_type in reversed(model.__mro__):
                roles = relation_type.__dict__.get("_roles", {})
                for role in roles.values():
                    for player in role.player_entity_types:
                        self._register_descriptor_closure(player, seen, subtype_roots)
                        self._register_loaded_subtypes(player, seen, subtype_roots)

        descriptor = descriptor_for_model(model)
        if issubclass(model, Relation):
            self.__registry.register_relation(descriptor)
        else:
            self.__registry.register_entity(descriptor)
        if existing is None:
            self.__models[type_name] = model

    def _register_loaded_subtypes(
        self,
        model: type[TypeDBType],
        seen: set[type[TypeDBType]],
        subtype_roots: set[type[TypeDBType]],
    ) -> None:
        if model in subtype_roots:
            return
        subtype_roots.add(model)
        for subtype in model.__subclasses__():
            if not isinstance(subtype, type) or not issubclass(subtype, TypeDBType):
                continue
            if not subtype.is_base():
                self._register_descriptor_closure(subtype, seen, subtype_roots)
            self._register_loaded_subtypes(subtype, seen, subtype_roots)


def _named_declaration(
    declaration: type[object],
) -> list[tuple[str, str, bool]]:
    if not isinstance(declaration, type):
        raise TypeError("query_as requires a frozen dataclass or NamedTuple class")

    if is_dataclass(declaration):
        parameters = getattr(declaration, "__dataclass_params__", None)
        if parameters is None or not parameters.frozen:
            raise TypeError("query_as dataclasses must be frozen")
        names = tuple(field.name for field in fields(declaration))
    elif issubclass(declaration, tuple) and isinstance(
        getattr(declaration, "_fields", None), tuple
    ):
        names = tuple(getattr(declaration, "_fields"))
        if any(not isinstance(name, str) for name in names):
            raise TypeError("query_as NamedTuple fields must have string names")
    else:
        raise TypeError("query_as requires a frozen dataclass or NamedTuple class")

    if len(set(names)) != len(names):
        raise TypeError("query_as declarations cannot contain duplicate field names")
    try:
        annotations: Mapping[str, object] = get_type_hints(declaration, include_extras=True)
    except (NameError, TypeError) as error:
        raise TypeError("query_as declaration annotations must resolve exactly") from error
    if tuple(annotations) != names:
        raise TypeError("query_as requires one exact annotation per declared field")
    return [(name, *_normalized_named_annotation(name, annotations[name])) for name in names]


def _normalized_named_annotation(name: str, annotation: object) -> tuple[str, bool]:
    origin = get_origin(annotation)
    if origin is tuple:
        arguments = get_args(annotation)
        if len(arguments) != 2 or arguments[1] is not Ellipsis:
            raise TypeError(
                f"query_as field {name!r} collection annotation must be tuple[Model, ...]"
            )
        model = arguments[0]
        collection = True
    else:
        model = annotation
        collection = False

    if not isinstance(model, type) or not issubclass(model, TypeDBType):
        raise TypeError(f"query_as field {name!r} annotation must be Model or tuple[Model, ...]")
    return model.get_type_name(), collection


__all__ = ["QuerySession"]
