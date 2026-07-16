from collections.abc import Mapping, Sequence
from typing import Generic, ParamSpec, TypeVar, overload

from type_bridge.session import Database, TransactionContext

_OwnerT = TypeVar("_OwnerT", bound="ModelBase")
_PlayerT_co = TypeVar("_PlayerT_co", covariant=True)
_ReadT_co = TypeVar("_ReadT_co", covariant=True)
_AssignT_contra = TypeVar("_AssignT_contra", contravariant=True)
_AccessOwnerT = TypeVar("_AccessOwnerT", bound="ModelBase")
_P = ParamSpec("_P")
_R_co = TypeVar("_R_co", covariant=True)
_ModelT = TypeVar("_ModelT", bound="ModelBase")

class FieldToken(Generic[_OwnerT]):
    owner: type[_OwnerT]
    fact: Mapping[str, object]

class RoleToken(Generic[_OwnerT, _PlayerT_co]):
    owner: type[_OwnerT]
    fact: Mapping[str, object]

class FunctionRef(Generic[_P, _R_co]):
    id: str
    signature: Mapping[str, object]
    def __init__(
        self,
        function_id: str,
        signature: Mapping[str, object],
    ) -> None: ...

class ModelBase:
    __projection__: Mapping[str, object]
    __runtime_projection__: object
    __type_id__: str
    __model_form__: str
    @property
    def iid(self) -> str | None: ...
    @classmethod
    def manager(
        cls: type[_ModelT],
        connection: Database | TransactionContext,
    ) -> ProjectedModelManager[_ModelT]: ...
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
class EntityBase(ModelBase): ...
class RelationBase(ModelBase): ...

class ProjectedModelManager(Generic[_ModelT]):
    def insert(self, instance: _ModelT) -> _ModelT: ...
    def all(self) -> list[_ModelT]: ...
    def get_by_iid(self, iid: str) -> _ModelT | None: ...

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

class FieldDescriptor(Generic[_OwnerT, _ReadT_co, _AssignT_contra]):
    @overload
    def __get__(
        self,
        instance: None,
        owner: type[_AccessOwnerT],
    ) -> FieldToken[_AccessOwnerT]: ...
    @overload
    def __get__(
        self,
        instance: _OwnerT,
        owner: type[_OwnerT] | None = ...,
    ) -> _ReadT_co: ...
    def __set__(
        self,
        instance: _OwnerT,
        value: _AssignT_contra,
    ) -> None: ...

class RoleDescriptor(
    Generic[_OwnerT, _PlayerT_co, _ReadT_co, _AssignT_contra]
):
    @overload
    def __get__(
        self,
        instance: None,
        owner: type[_AccessOwnerT],
    ) -> RoleToken[_AccessOwnerT, _PlayerT_co]: ...
    @overload
    def __get__(
        self,
        instance: _OwnerT,
        owner: type[_OwnerT] | None = ...,
    ) -> _ReadT_co: ...
    def __set__(
        self,
        instance: _OwnerT,
        value: _AssignT_contra,
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
