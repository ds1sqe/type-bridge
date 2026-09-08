from collections.abc import Callable, Mapping, Sequence
from enum import Enum
from typing import Literal, Never, overload

from type_bridge_core import PyRuntimeProjection

from type_bridge._runtime_projection import GeneratedEntityProjection, GeneratedRelationProjection
from type_bridge.session import Database, TransactionContext

from ._query import QuerySession

type _OwnerLookup = Literal[
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
]

class FieldToken[OwnerT: ModelBase, AttributeT: AttributeBase]:
    owner: type[OwnerT]
    fact: Mapping[str, object]

class RoleToken[OwnerT: ModelBase, PlayerT_co: ModelBase, CompatibleBindingT_contra]:
    owner: type[OwnerT]
    fact: Mapping[str, object]
    def _accepts_binding(self, binding: CompatibleBindingT_contra) -> None: ...

class FunctionRef[**P, R_co]:
    id: str
    signature: Mapping[str, object]
    def __init__(
        self,
        function_id: str,
        signature: Mapping[str, object],
    ) -> None: ...

class ModelBase:
    __projection__: Mapping[str, object]
    __runtime_projection__: PyRuntimeProjection
    __type_id__: str
    __model_form__: str
    @property
    def iid(self) -> str | None: ...
    @classmethod
    def manager[ModelT: ModelBase](
        cls: type[ModelT],
        connection: Database | TransactionContext,
    ) -> ProjectedModelManager[ModelT]: ...
    @classmethod
    def query[ModelT: ModelBase](
        cls: type[ModelT],
        connection: Database | TransactionContext,
    ) -> QuerySession: ...
    def runtime_values(self) -> dict[str, object]: ...
    def initialize_runtime_values(
        self,
        values: Mapping[str, object],
    ) -> None: ...
    def attach_runtime_iid(self, iid: str) -> None: ...

class AttributeBase(ModelBase):
    @property
    def value(self) -> object: ...
    def runtime_attribute_value(self) -> object: ...
    def initialize_runtime_attribute(self, value: object, scalar: str) -> None: ...
    @classmethod
    def _from_validated_query_value(cls, value: object) -> AttributeBase: ...
    @classmethod
    @overload
    def owners[AttributeT: AttributeBase](
        cls: type[AttributeT],
        connection: Database | TransactionContext,
        value: object = ...,
        *,
        kind: Literal["entity"],
        lookup: _OwnerLookup = ...,
    ) -> list[EntityBase]: ...
    @classmethod
    @overload
    def owners[AttributeT: AttributeBase](
        cls: type[AttributeT],
        connection: Database | TransactionContext,
        value: object = ...,
        *,
        kind: Literal["relation"],
        lookup: _OwnerLookup = ...,
    ) -> list[RelationBase]: ...

def attribute_model_for_query_label(label: str) -> type[AttributeBase]: ...

class LongAttributeBase(AttributeBase):
    @property
    def value(self) -> int: ...

class DoubleAttributeBase(AttributeBase):
    @property
    def value(self) -> float: ...

class EntityBase(ModelBase, GeneratedEntityProjection):
    @classmethod
    def has[AttributeT: AttributeBase](
        cls,
        connection: Database | TransactionContext,
        attribute: type[AttributeT],
        value: object = ...,
        *,
        lookup: _OwnerLookup = ...,
    ) -> list[EntityBase]: ...

class RelationBase(ModelBase, GeneratedRelationProjection):
    @classmethod
    def has[AttributeT: AttributeBase](
        cls,
        connection: Database | TransactionContext,
        attribute: type[AttributeT],
        value: object = ...,
        *,
        lookup: _OwnerLookup = ...,
    ) -> list[RelationBase]: ...

class CrudEvent(Enum):
    PRE_INSERT = "pre_insert"
    POST_INSERT = "post_insert"
    PRE_UPDATE = "pre_update"
    POST_UPDATE = "post_update"
    PRE_DELETE = "pre_delete"
    POST_DELETE = "post_delete"
    PRE_PUT = "pre_put"
    POST_PUT = "post_put"

class HookCancelled(Exception):  # noqa: N818 - stable generated runtime API
    reason: str
    event: CrudEvent | None
    hook: CrudHook[Never] | None
    def __init__(
        self,
        reason: str = "",
        *,
        event: CrudEvent | None = None,
        hook: CrudHook[Never] | None = None,
    ) -> None: ...

class CrudHook[ModelT: ModelBase]:
    def should_run(self, event: CrudEvent, sender: type[ModelT]) -> bool: ...
    def pre_insert(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def post_insert(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def pre_update(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def post_update(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def pre_delete(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def post_delete(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def pre_put(self, sender: type[ModelT], instance: ModelT) -> None: ...
    def post_put(self, sender: type[ModelT], instance: ModelT) -> None: ...

class ProjectedModelNotFoundError(LookupError): ...

class ProjectedModelManager[ModelT: ModelBase]:
    def add_hook(self, hook: CrudHook[ModelT]) -> ProjectedModelManager[ModelT]: ...
    def remove_hook(self, hook: CrudHook[ModelT]) -> None: ...
    def insert(self, instance: ModelT) -> ModelT: ...
    def insert_many(self, instances: Sequence[ModelT]) -> list[ModelT]: ...
    def put(self, instance: ModelT) -> ModelT: ...
    def put_many(self, instances: Sequence[ModelT]) -> list[ModelT]: ...
    def update(self, instance: ModelT) -> ModelT: ...
    def update_many(self, instances: Sequence[ModelT]) -> list[ModelT]: ...
    @overload
    def delete(self, instance_or_iid: ModelT) -> ModelT: ...
    @overload
    def delete(self, instance_or_iid: str) -> None: ...
    @overload
    def delete(self) -> int: ...
    def delete_many(
        self,
        instances: Sequence[ModelT],
        *,
        strict: bool = False,
    ) -> list[ModelT]: ...
    def update_with(self, function: Callable[[ModelT], None]) -> list[ModelT]: ...
    def filter(self, **filters: object) -> ProjectedModelManager[ModelT]: ...
    def all(self) -> list[ModelT]: ...
    def first(self) -> ModelT | None: ...
    def count(self) -> int: ...
    def exists(self) -> bool: ...
    def get_by_iid(self, iid: str) -> ModelT | None: ...

class ReferenceBase:
    __projection__: Mapping[str, object]
    __type_id__: str
    __model_form__: str
    @property
    def iid(self) -> str: ...
    def runtime_values(self) -> dict[str, object]: ...
    def initialize_runtime_reference(
        self,
        iid: str,
        values: Mapping[str, object],
    ) -> None: ...

class StructValueBase:
    __struct_id__: str

class FieldDescriptor[
    OwnerT: ModelBase,
    AttributeT: AttributeBase,
    ReadT_co,
    AssignT_contra,
]:
    @overload
    def __get__[AccessOwnerT: ModelBase](
        self,
        instance: None,
        owner: type[AccessOwnerT],
    ) -> FieldToken[AccessOwnerT, AttributeT]: ...
    @overload
    def __get__(
        self,
        instance: OwnerT,
        owner: type[OwnerT] | None = ...,
    ) -> ReadT_co: ...
    def __set__(
        self,
        instance: OwnerT,
        value: AssignT_contra,
    ) -> None: ...

class RoleDescriptor[
    OwnerT: ModelBase,
    PlayerT_co: ModelBase,
    CompatibleBindingT_contra,
    ReadT_co,
    AssignT_contra,
]:
    @overload
    def __get__[AccessOwnerT: ModelBase](
        self,
        instance: None,
        owner: type[AccessOwnerT],
    ) -> RoleToken[AccessOwnerT, PlayerT_co, CompatibleBindingT_contra]: ...
    @overload
    def __get__(
        self,
        instance: OwnerT,
        owner: type[OwnerT] | None = ...,
    ) -> ReadT_co: ...
    def __set__(
        self,
        instance: OwnerT,
        value: AssignT_contra,
    ) -> None: ...

def load_mapping(source: str) -> Mapping[str, object]: ...
def initialize_model(
    instance: ModelBase,
    values: Mapping[str, object],
) -> None: ...
def initialize_reference(
    instance: ReferenceBase,
    iid: str,
    values: Mapping[str, object],
) -> None: ...
def initialize_attribute(
    instance: AttributeBase,
    value: object,
    scalar: str,
) -> None: ...
def freeze_struct(
    instance: StructValueBase,
    values: Mapping[str, object],
) -> None: ...
def install_model(
    owner: type[ModelBase],
    reference: type[ReferenceBase] | None,
    projection: Mapping[str, object],
) -> None: ...
def install_runtime_projection(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[ModelBase], type[ReferenceBase] | None]],
) -> None: ...
