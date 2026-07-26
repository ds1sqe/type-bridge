"""Transport-neutral remote typed-query session over released composition."""

from __future__ import annotations

from typing import Literal, TypeVar, overload

from type_bridge_core import QueryV2Authority, query_v2_remote_model_context

from type_bridge.fields.role import RoleRef
from type_bridge.models.base import TypeDBType
from type_bridge.models.relation import Relation
from type_bridge.typed._remote_terminal import (
    RemoteQueryExchange,
    _RemoteRuntime,
)
from type_bridge.typed.references import BoundVar, Predicate, Selection, _PlayerBinding
from type_bridge.typed.remote_limits import RemoteQueryLimits
from type_bridge.typed.remote_query import RemoteQuery
from type_bridge.typed.session import QuerySession

ModelT = TypeVar("ModelT", bound=TypeDBType)


class RemoteQuerySession:
    """Compose with the released grammar and execute through caller transport."""

    __slots__ = ("__direct", "__runtime")

    def __init__(
        self,
        authority: QueryV2Authority,
        advertisement: bytes,
        exchange: RemoteQueryExchange,
        limits: RemoteQueryLimits,
    ) -> None:
        if not isinstance(authority, QueryV2Authority):
            raise TypeError("RemoteQuerySession requires a QueryV2Authority")
        if type(advertisement) is not bytes:
            raise TypeError("RemoteQuerySession advertisement must be exact bytes")
        if not callable(exchange):
            raise TypeError("RemoteQuerySession exchange must be callable")
        if not isinstance(limits, RemoteQueryLimits):
            raise TypeError("RemoteQuerySession limits must be RemoteQueryLimits")

        context = query_v2_remote_model_context(
            authority,
            bytes(advertisement),
            limits.max_items,
            limits.max_bytes,
            limits.max_collection_members,
            limits.max_graph_nodes,
            limits.max_attribute_values,
            limits.max_role_players,
            limits.deadline_ms,
        )
        self.__direct = QuerySession._diagnostic()
        self.__runtime = _RemoteRuntime(context, exchange)

    @overload
    def var(self, model: type[ModelT], *, subtypes: Literal[False] = False) -> BoundVar[ModelT]: ...

    @overload
    def var(self, model: type[ModelT], *, subtypes: Literal[True]) -> BoundVar[ModelT]: ...

    def var(self, model: type[ModelT], *, subtypes: bool = False) -> BoundVar[ModelT]:
        """Delegate model registration and binding to the released session."""
        return self.__direct.var(model, subtypes=subtypes)

    def exact(self, model: type[ModelT]) -> BoundVar[ModelT]:
        """Create one exact binding through the released session."""
        return self.__direct.exact(model)

    def subtypes(self, model: type[ModelT]) -> BoundVar[ModelT]:
        """Create one subtype-inclusive binding through the released session."""
        return self.__direct.subtypes(model)

    def reachable[
        SourcePlayerT: TypeDBType,
        TargetPlayerT: TypeDBType,
        RelationT: Relation,
    ](
        self,
        source: _PlayerBinding[SourcePlayerT],
        target: _PlayerBinding[TargetPlayerT],
        relation: type[RelationT],
        role_from: RoleRef[SourcePlayerT, RelationT],
        role_to: RoleRef[TargetPlayerT, RelationT],
        *,
        min_depth: int,
        max_depth: int,
    ) -> Predicate:
        """Delegate bounded reachability to the released session."""
        return self.__direct.reachable(
            source,
            target,
            relation,
            role_from,
            role_to,
            min_depth=min_depth,
            max_depth=max_depth,
        )

    # BEGIN GENERATED REMOTE QUERY OVERLOADS
    @overload
    def query[T1](
        self,
        selection1: Selection[T1],
        /,
    ) -> RemoteQuery[T1]: ...

    @overload
    def query[T1, T2](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        /,
    ) -> RemoteQuery[T1, T2]: ...

    @overload
    def query[T1, T2, T3](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        /,
    ) -> RemoteQuery[T1, T2, T3]: ...

    @overload
    def query[T1, T2, T3, T4](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4]: ...

    @overload
    def query[T1, T2, T3, T4, T5](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]: ...

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
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16]: ...

    # END GENERATED REMOTE QUERY OVERLOADS

    def query(self, *selections: Selection[object]) -> object:
        """Wrap one ordinary immutable positional query for remote terminals."""
        direct = self.__direct.query(*selections)
        return RemoteQuery._from_direct(direct, self.__runtime)

    def query_as[DeclaredRowT](
        self,
        declaration: type[DeclaredRowT],
        /,
        **selections: Selection[object],
    ) -> RemoteQuery[DeclaredRowT]:
        """Wrap one released named query for remote terminals."""
        direct = self.__direct.query_as(declaration, **selections)
        return RemoteQuery._from_direct(direct, self.__runtime)


__all__ = ["RemoteQuerySession"]
