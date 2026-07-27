from collections.abc import Mapping, Sequence
from typing import overload

from type_bridge.session import Database, TransactionContext

class FieldToken[OwnerT: ModelBase]:
    owner: type[OwnerT]
    fact: Mapping[str, object]

class RoleToken[OwnerT: ModelBase, PlayerT_co]:
    owner: type[OwnerT]
    fact: Mapping[str, object]

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
    __runtime_projection__: object
    __type_id__: str
    __model_form__: str
    @property
    def iid(self) -> str | None: ...
    @classmethod
    def manager[ModelT: ModelBase](
        cls: type[ModelT],
        connection: Database | TransactionContext,
    ) -> ProjectedModelManager[ModelT]: ...
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

class ProjectedModelManager[ModelT: ModelBase]:
    def insert(self, instance: ModelT) -> ModelT: ...
    def all(self) -> list[ModelT]: ...
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

class FieldDescriptor[OwnerT: ModelBase, ReadT_co, AssignT_contra]:
    @overload
    def __get__[AccessOwnerT: ModelBase](
        self,
        instance: None,
        owner: type[AccessOwnerT],
    ) -> FieldToken[AccessOwnerT]: ...
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

class RoleDescriptor[OwnerT: ModelBase, PlayerT_co, ReadT_co, AssignT_contra]:
    @overload
    def __get__[AccessOwnerT: ModelBase](
        self,
        instance: None,
        owner: type[AccessOwnerT],
    ) -> RoleToken[AccessOwnerT, PlayerT_co]: ...
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
