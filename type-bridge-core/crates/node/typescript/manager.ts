import {
  type AttributeInput,
  type AttributeValue,
  type DynamicRelationRow,
  type DynamicRolePlayer,
  type EntityDescriptor,
  type RelationDescriptor,
  type RolePlayerInput,
  type RustDatabase,
  type RustDynamicEntityManager,
  type RustDynamicRelationManager,
  type RustTransactionContext,
} from "./index.js";
import { type Attribute } from "./attribute.js";
import {
  hydrateAttributeEntries,
  hydrateAttributes,
  keyAttributeDescriptor,
  lowerAttributes,
  lowerAttributeValue,
  lowerFilters,
  runtimeAttributeValueFromUnknown,
  TypedCodecError,
} from "./codec.js";
import { setIid, type IidBearing } from "./iid.js";
import type { EntitySchema, RelationSchema, RoleSpec, SchemaSpec } from "./model.js";

export type ManagerConnection = RustDatabase | RustTransactionContext;

export type ExactFilters<T> = Partial<{
  readonly [Key in keyof T as NonNullable<T[Key]> extends Attribute<unknown, string> ? Key : never]: NonNullable<T[Key]>;
}>;

type EntityConstructor<T extends IidBearing> = (new (
  values: Record<string, unknown>,
) => T) & {
  readonly schema: EntitySchema;
  descriptor(): EntityDescriptor;
};

type RelationConstructor<T extends IidBearing> = (new (
  values: Record<string, unknown>,
) => T) & {
  readonly schema: RelationSchema;
  descriptor(): RelationDescriptor;
};

type ModelConstructor<T extends IidBearing> =
  | EntityConstructor<T>
  | RelationConstructor<T>;

type RoleLike = {
  readonly kind: "role";
  readonly players: readonly ModelTokenLike[];
  readonly cardinality: [number, number | null] | null;
};

type ModelTokenLike =
  | string
  | ((new (values: Record<string, unknown>) => IidBearing) & {
      readonly typeName: string;
      readonly schema: EntitySchema;
    });

export class TypedEntityManager<T extends IidBearing> {
  readonly #modelClass: EntityConstructor<T>;
  readonly #dynamic: RustDynamicEntityManager;

  constructor(modelClass: EntityConstructor<T>, connection: ManagerConnection) {
    this.#modelClass = modelClass;
    this.#dynamic = connection.entityManager(modelClass.descriptor());
  }

  insert(instance: T): T {
    return setIid(instance, this.#dynamic.insert(lowerAttributes(instance, this.#modelClass.schema)));
  }

  insertMany(instances: readonly T[]): T[] {
    const iids = this.#dynamic.insertMany(
      instances.map((instance) => lowerAttributes(instance, this.#modelClass.schema)),
    );
    return setIids(instances, iids);
  }

  put(instance: T): T {
    return setIid(instance, this.#dynamic.put(lowerAttributes(instance, this.#modelClass.schema)));
  }

  putMany(instances: readonly T[]): T[] {
    const iids = this.#dynamic.putMany(
      instances.map((instance) => lowerAttributes(instance, this.#modelClass.schema)),
    );
    return setIids(instances, iids);
  }

  update(instance: T): T {
    this.#dynamic.update(lowerAttributes(instance, this.#modelClass.schema), instance._iid);
    return instance;
  }

  get(filters?: ExactFilters<T> | null): T[] {
    return this.#dynamic
      .get(lowerTypedFilters(filters, this.#modelClass.schema))
      .map((row) => hydrateEntity(this.#modelClass, row));
  }

  all(): T[] {
    return this.#dynamic.all().map((row) => hydrateEntity(this.#modelClass, row));
  }

  getByIid(iid: string): T | null {
    const row = this.#dynamic.getByIid(iid);
    return row === null ? null : hydrateEntity(this.#modelClass, row);
  }

  count(filters?: ExactFilters<T> | null): bigint {
    return this.#dynamic.count(lowerTypedFilters(filters, this.#modelClass.schema));
  }

  delete(instanceOrIid: T | string): void {
    this.#dynamic.deleteByIid(resolveIid(instanceOrIid, "entity"));
  }
}

export class TypedRelationManager<T extends IidBearing> {
  readonly #modelClass: RelationConstructor<T>;
  readonly #dynamic: RustDynamicRelationManager;

  constructor(modelClass: RelationConstructor<T>, connection: ManagerConnection) {
    this.#modelClass = modelClass;
    this.#dynamic = connection.relationManager(modelClass.descriptor());
  }

  insert(instance: T): T {
    const iid = this.#dynamic.insert(
      lowerAttributes(instance, this.#modelClass.schema),
      buildRolePlayers(instance, this.#modelClass.schema),
    );
    return setIid(instance, iid);
  }

  insertMany(instances: readonly T[]): T[] {
    const iids = this.#dynamic.insertMany(
      instances.map((instance) => ({
        attributes: lowerAttributes(instance, this.#modelClass.schema),
        role_players: buildRolePlayers(instance, this.#modelClass.schema),
      })),
    );
    return setIids(instances, iids);
  }

  put(instance: T): T {
    const iid = this.#dynamic.put(
      lowerAttributes(instance, this.#modelClass.schema),
      buildRolePlayers(instance, this.#modelClass.schema),
    );
    return setIid(instance, iid);
  }

  putMany(instances: readonly T[]): T[] {
    const iids = this.#dynamic.putMany(
      instances.map((instance) => ({
        attributes: lowerAttributes(instance, this.#modelClass.schema),
        role_players: buildRolePlayers(instance, this.#modelClass.schema),
      })),
    );
    return setIids(instances, iids);
  }

  update(instance: T): T {
    this.#dynamic.update(
      lowerAttributes(instance, this.#modelClass.schema),
      buildRolePlayers(instance, this.#modelClass.schema),
      instance._iid,
    );
    return instance;
  }

  get(filters?: ExactFilters<T> | null): T[] {
    return hydrateRelationRows(
      this.#modelClass,
      this.#dynamic.get(lowerTypedFilters(filters, this.#modelClass.schema)),
    );
  }

  all(): T[] {
    return hydrateRelationRows(this.#modelClass, this.#dynamic.all());
  }

  getByIid(iid: string): T | null {
    const rows = hydrateRelationRows(this.#modelClass, this.#dynamic.getByIid(iid));
    return rows[0] ?? null;
  }

  count(filters?: ExactFilters<T> | null): bigint {
    return this.#dynamic.count(lowerTypedFilters(filters, this.#modelClass.schema));
  }

  delete(instanceOrIid: T | string): void {
    this.#dynamic.deleteByIid(resolveIid(instanceOrIid, "relation"));
  }
}

export function entityManagerFor<T extends IidBearing>(
  modelClass: EntityConstructor<T>,
  connection: ManagerConnection,
): TypedEntityManager<T> {
  return new TypedEntityManager(modelClass, connection);
}

export function relationManagerFor<T extends IidBearing>(
  modelClass: RelationConstructor<T>,
  connection: ManagerConnection,
): TypedRelationManager<T> {
  return new TypedRelationManager(modelClass, connection);
}

export function buildRolePlayers(instance: IidBearing, schema: RelationSchema): RolePlayerInput[] {
  const source = instance as unknown as Record<string, unknown>;
  const inputs: RolePlayerInput[] = [];
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (!isRoleSpec(spec)) {
      continue;
    }
    const value = source[fieldName];
    if (value === undefined || value === null) {
      continue;
    }
    const players = Array.isArray(value) ? value : [value];
    for (const player of players) {
      inputs.push(rolePlayerInput(fieldName, spec, player));
    }
  }
  return inputs;
}

function hydrateEntity<T extends IidBearing>(
  modelClass: EntityConstructor<T>,
  row: { iid: string | null; attributes: [string, import("./index.js").RuntimeAttributeValue][] },
): T {
  return setIid(new modelClass(hydrateAttributes(row, modelClass.schema)), row.iid);
}

function hydrateRelationRows<T extends IidBearing>(
  modelClass: RelationConstructor<T>,
  rows: DynamicRelationRow[],
): T[] {
  return Array.from(groupRelationRows(rows).values()).map((group) =>
    hydrateRelationGroup(modelClass, group),
  );
}

function hydrateRelationGroup<T extends IidBearing>(
  modelClass: RelationConstructor<T>,
  rows: DynamicRelationRow[],
): T {
  const values: Record<string, unknown> = {
    ...hydrateAttributes(rows[0], modelClass.schema),
    ...hydrateRoleFields(modelClass.schema, rows),
  };
  return setIid(new modelClass(values), rows[0]?.iid ?? null);
}

function hydrateRoleFields(schema: RelationSchema, rows: DynamicRelationRow[]): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  const seen = new Set<string>();
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (!isRoleSpec(spec)) {
      continue;
    }
    const players: IidBearing[] = [];
    for (const row of rows) {
      for (const player of row.role_players) {
        if (player.role_name !== fieldName) {
          continue;
        }
        const hydrated = hydrateRolePlayer(spec, player);
        const key = `${fieldName}:${hydrated._iid ?? JSON.stringify(hydrated)}`;
        if (seen.has(key)) {
          continue;
        }
        seen.add(key);
        players.push(hydrated);
      }
    }
    if (players.length === 0) {
      if (isMultiRole(spec)) {
        values[fieldName] = [];
      }
      continue;
    }
    values[fieldName] = isMultiRole(spec) || players.length > 1 ? players : players[0];
  }
  return values;
}

function hydrateRolePlayer(spec: RoleLike, player: DynamicRolePlayer): IidBearing {
  const modelClass = modelClassForPlayer(spec, player);
  const entries = player.attributes.map(([name, value]) => [
    name,
    runtimeAttributeValueFromUnknown(value, valueTypeForAttributeName(modelClass.schema, name)),
  ] as const);
  return setIid(
    new modelClass(hydrateAttributeEntries(entries, modelClass.schema)),
    player.player_iid,
  );
}

function modelClassForPlayer(spec: RoleLike, player: DynamicRolePlayer): Exclude<ModelTokenLike, string> {
  const classes = spec.players.filter(isModelClass);
  const matching = classes.find((modelClass) => modelClass.typeName === player.player_type_name);
  if (matching !== undefined) {
    return matching;
  }
  if (classes.length === 1) {
    return classes[0];
  }
  throw new TypedCodecError(
    `Cannot hydrate role "${player.role_name}" player type "${player.player_type_name ?? "<unknown>"}"`,
  );
}

function rolePlayerInput(fieldName: string, spec: RoleLike, player: unknown): RolePlayerInput {
  if (!isModelInstance(player)) {
    throw new TypedCodecError(`Role field "${fieldName}" must contain typed model instances`);
  }
  const modelClass = player.constructor as ModelConstructor<IidBearing>;
  const input: RolePlayerInput = {
    role_name: fieldName,
    player_type_name: modelClass.descriptor().type_name,
  };
  if (player._iid !== null) {
    return { ...input, iid: player._iid };
  }
  const key = keyAttributeDescriptor(modelClass.schema as EntitySchema);
  if (key === null) {
    throw new TypedCodecError(`Role player for role "${fieldName}" needs _iid or a key attribute`);
  }
  const keyValue = (player as unknown as Record<string, unknown>)[key.field_name];
  if (!isAttributeLike(keyValue)) {
    throw new TypedCodecError(`Role player key field "${key.field_name}" is missing`);
  }
  return {
    ...input,
    key_attr: key.attr_name,
    key_value: lowerAttributeValue(key.value_type, keyValue.value),
  };
}

function lowerTypedFilters<T extends IidBearing>(
  filters: ExactFilters<T> | null | undefined,
  schema: EntitySchema | RelationSchema,
): Record<string, AttributeValue> | null {
  return lowerFilters(filters as Record<string, Attribute<unknown, string>> | null | undefined, schema);
}

function groupRelationRows(rows: DynamicRelationRow[]): Map<string, DynamicRelationRow[]> {
  const grouped = new Map<string, DynamicRelationRow[]>();
  for (const [index, row] of rows.entries()) {
    const key = row.iid ?? `__row_${index}`;
    const group = grouped.get(key);
    if (group === undefined) {
      grouped.set(key, [row]);
    } else {
      group.push(row);
    }
  }
  return grouped;
}

function setIids<T extends IidBearing>(instances: readonly T[], iids: string[]): T[] {
  if (instances.length !== iids.length) {
    throw new TypedCodecError(`Rust runtime returned ${iids.length} IIDs for ${instances.length} instances`);
  }
  return instances.map((instance, index) => setIid(instance, iids[index]));
}

function resolveIid<T extends IidBearing>(instanceOrIid: T | string, kind: string): string {
  if (typeof instanceOrIid === "string") {
    return instanceOrIid;
  }
  if (instanceOrIid._iid === null) {
    throw new TypedCodecError(`Cannot delete ${kind} instance without _iid`);
  }
  return instanceOrIid._iid;
}

function isRoleSpec(spec: SchemaSpec): spec is RoleSpec<readonly ModelTokenLike[]> & RoleLike {
  return spec.kind === "role";
}

function valueTypeForAttributeName(schema: EntitySchema, attrName: string) {
  for (const spec of Object.values(schema)) {
    if (spec.kind === "field" && spec.attrType.attrName === attrName) {
      return spec.attrType.valueType;
    }
  }
  return undefined;
}

function isMultiRole(spec: RoleLike): boolean {
  return spec.cardinality !== null && (spec.cardinality[1] === null || spec.cardinality[1] > 1);
}

function isModelClass(value: ModelTokenLike): value is Exclude<ModelTokenLike, string> {
  return typeof value === "function";
}

function isModelInstance(value: unknown): value is IidBearing {
  return typeof value === "object" && value !== null && "_iid" in value;
}

function isAttributeLike(value: unknown): value is Attribute<unknown, string> {
  return typeof value === "object" && value !== null && "value" in value;
}
