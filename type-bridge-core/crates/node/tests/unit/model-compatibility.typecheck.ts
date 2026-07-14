import {
  Entity,
  Relation,
  type EntityDescriptor,
  type ModelClass,
  type ModelInstance,
  type ParentModelClass,
  type ResolvedTypeFlags,
} from "../../typescript/index.js";

function assertLegacyFactoryInputs(typeNameOrFlags: string | ResolvedTypeFlags): void {
  const ParentEntity = Entity(typeNameOrFlags, {});
  const ParentRelation = Relation(typeNameOrFlags, {});

  const ChildEntity = Entity(typeNameOrFlags, {}, { parent: ParentEntity });
  const ChildRelation = Relation(typeNameOrFlags, {}, { parent: ParentRelation });
  const publicEntityClass: ModelClass<{}, EntityDescriptor> = ParentEntity;
  const publicParentClass: ParentModelClass<{}> = ParentEntity;

  void ChildEntity;
  void ChildRelation;
  void publicEntityClass;
  void publicParentClass;
}

function assertLegacyStructuralAliases(
  instanceShape: Readonly<{
    _iid: string | null;
    toDict(): {};
  }>,
  parentShape: (new (values: never) => object) & {
    readonly typeName: string;
    readonly schema: {};
  },
  modelShape: (new (values: {}) => ModelInstance<{}>) & Pick<
    ModelClass<{}, EntityDescriptor>,
    "typeName" | "schema" | "flags" | "descriptor" | "fromDict" | "manager"
  >,
): void {
  const instance: ModelInstance<{}> = instanceShape;
  const parent: ParentModelClass<{}> = parentShape;
  const model: ModelClass<{}, EntityDescriptor> = modelShape;

  void instance;
  void parent;
  void model;
}

void assertLegacyFactoryInputs;
void assertLegacyStructuralAliases;
