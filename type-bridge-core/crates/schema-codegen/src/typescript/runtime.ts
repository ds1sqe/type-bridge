import {
  installGeneratedSchemaAuthority,
  installRuntimeProjection,
  type InstalledRuntimeProjection,
  type NativeProjectedManager,
  type RuntimeProjectionConnection,
  type RuntimeProjectionMatchBinding,
  type RuntimeProjectionMatchField,
  type RuntimeProjectionMatchOrder,
  type RuntimeProjectionMatchPredicate,
  type RuntimeProjectionMatchQuery,
  type RuntimeProjectionMatchResult,
  type RuntimeProjectionReduction,
  type RuntimeProjectionMatchSelection,
  type RuntimeProjectionMatchSession,
  type RuntimeProjectionMatchThing,
  type RuntimeProjectionRemote,
  type RuntimeProjectionRemoteExchange,
  type RuntimeProjectionRemoteLimits,
} from "@type-bridge/node/runtime-projection";
import type { QueryV2Authority } from "@type-bridge/node/query-v2";

const COMPLETE_BRAND: unique symbol = Symbol("typebridge.complete");
const REFERENCE_BRAND: unique symbol = Symbol("typebridge.reference");
const STRUCT_BRAND: unique symbol = Symbol("typebridge.struct");
const TYPE_TOKEN_BRAND: unique symbol = Symbol("typebridge.type-token");
const FIELD_TOKEN_BRAND: unique symbol = Symbol("typebridge.field-token");
const ROLE_TOKEN_BRAND: unique symbol = Symbol("typebridge.role-token");
const PLAYS_TOKEN_BRAND: unique symbol = Symbol("typebridge.plays-token");
const FUNCTION_TOKEN_BRAND: unique symbol = Symbol("typebridge.function-token");
const HYDRATE_COMPLETE_BRAND: unique symbol = Symbol("typebridge.hydrate-complete");

export interface Cardinality {
  readonly kind: "cardinality";
  readonly min: string;
  readonly max: string | null;
}

export type ProjectedModelForm = "complete" | "reference";

export interface Multiplicity {
  readonly cardinality: Cardinality;
  readonly required: boolean;
  readonly container: "scalar" | "sequence";
}

export interface ModelUse {
  readonly typeKey: string;
  readonly form: ProjectedModelForm;
}

export interface CompleteFacet<Id extends string> {
  readonly __typebridgeModel: Id;
  readonly __typebridgeForm: "complete";
  readonly iid: string | null;
  readonly [COMPLETE_BRAND]: Id;
}

/** JSON-safe values accepted by generated manager lookup filters. */
export type ProjectedManagerFilterValue =
  | string
  | number
  | boolean
  | null
  | CompleteFacet<string>
  | ReferenceFacet<string>
  | readonly ProjectedManagerFilterValue[];

export interface ProjectedModelManager<Complete> {
  insert(instance: Complete): Complete;
  insertMany(instances: readonly Complete[]): readonly Complete[];
  put(instance: Complete): Complete;
  putMany(instances: readonly Complete[]): readonly Complete[];
  update(iid: string, replacement: Complete): Complete;
  delete(instanceOrIid: Complete | string): void;
  filter(
    filters: Readonly<Record<string, ProjectedManagerFilterValue>>,
  ): ProjectedModelManager<Complete>;
  getByIid(iid: string): Complete | null;
  all(): readonly Complete[];
  first(): Complete | null;
  count(): bigint;
  exists(): boolean;
}

export interface ReferenceFacet<Id extends string> {
  readonly __typebridgeModel: Id;
  readonly __typebridgeForm: "reference";
  readonly iid: string;
  readonly [REFERENCE_BRAND]: Id;
}

export interface StructValue<Id extends string> {
  readonly __typebridgeStruct: Id;
  readonly [STRUCT_BRAND]: Id;
}

export interface TypeToken<Id extends string> {
  readonly kind: "type";
  readonly typeKey: Id;
  readonly id: unknown;
  readonly [TYPE_TOKEN_BRAND]: Id;
}

export interface FieldToken<Owner extends string, Attribute extends string, Value> {
  readonly kind: "field";
  readonly owner: Owner;
  readonly attribute: Attribute;
  readonly name: string;
  readonly multiplicity: Multiplicity;
  readonly key: boolean;
  readonly unique: boolean;
  readonly metadata: unknown;
  readonly [FIELD_TOKEN_BRAND]: (value: Value) => readonly [Owner, Attribute];
}

export interface RoleToken<
  Owner extends string,
  Role extends string,
  Player,
  SubtypeRoot = Player,
> {
  readonly kind: "role";
  readonly owner: Owner;
  readonly role: Role;
  readonly name: string;
  readonly acceptedPlayers: readonly string[];
  readonly specializes: unknown | null;
  readonly multiplicity: Multiplicity;
  readonly abstract: boolean;
  readonly metadata: unknown;
  readonly [ROLE_TOKEN_BRAND]: (
    player: Player,
    subtypeRoot: SubtypeRoot,
  ) => readonly [Owner, Role];
}

export interface PlaysToken<Player extends string, Role extends string> {
  readonly kind: "plays";
  readonly player: Player;
  readonly role: Role;
  readonly name: string;
  readonly multiplicity: Multiplicity;
  readonly metadata: unknown;
  readonly [PLAYS_TOKEN_BRAND]: readonly [Player, Role];
}

export interface FunctionToken<
  Id extends string,
  Arguments extends readonly unknown[],
  Result,
> {
  readonly kind: "function";
  readonly id: Id;
  readonly name: string;
  readonly metadata: unknown;
  readonly [FUNCTION_TOKEN_BRAND]: (arguments_: Arguments) => Result;
}

export type ModelToken<
  Id extends string,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields extends object,
  Roles extends object,
> = TypeToken<Id> & Fields & Roles & {
  readonly name: string;
  readonly fields: Fields;
  readonly roles: Roles;
  readonly valueType: ScalarValueType | null;
  readonly create: CreateFactory;
  readonly reference: ReferenceFactory;
  readonly manager: (connection: RuntimeProjectionConnection) => ProjectedModelManager<Complete>;
  readonly metadata: unknown;
  readonly __complete?: Complete;
  readonly [HYDRATE_COMPLETE_BRAND]: (iid: string | null, input: unknown) => Complete;
};

export type StructFactory<Id extends string, Value, Input> = {
  (input: Input): Value;
  readonly id: Id;
  readonly metadata: unknown;
};

interface FieldTokenDefinition<Owner extends string, Attribute extends string> {
  readonly owner: Owner;
  readonly attribute: Attribute;
  readonly name: string;
  readonly multiplicity: Multiplicity;
  readonly key: boolean;
  readonly unique: boolean;
  readonly metadata: unknown;
}

interface RoleTokenDefinition<Owner extends string, Role extends string> {
  readonly owner: Owner;
  readonly role: Role;
  readonly name: string;
  readonly acceptedPlayers: readonly string[];
  readonly specializes: unknown | null;
  readonly multiplicity: Multiplicity;
  readonly abstract: boolean;
  readonly metadata: unknown;
}

interface CreateMemberDefinition {
  readonly name: string;
  readonly multiplicity: Multiplicity;
  readonly accepted?: readonly ModelUse[];
}

type ScalarValueType =
  | "string"
  | "long"
  | "double"
  | "boolean"
  | "date"
  | "datetime"
  | "datetime_tz"
  | "decimal"
  | "duration";

interface ModelDefinition<Id extends string, Fields extends object, Roles extends object> {
  readonly typeKey: Id;
  readonly id: unknown;
  readonly name: string;
  readonly valueType: ScalarValueType | null;
  readonly fields: Fields;
  readonly roles: Roles;
  readonly completeMembers: readonly CreateMemberDefinition[];
  readonly createEnabled: boolean;
  readonly createMembers: readonly CreateMemberDefinition[];
  readonly referenceEnabled: boolean;
  readonly referenceKeys: readonly string[];
  readonly metadata: unknown;
}

interface RuntimeModelToken {
  readonly typeKey: string;
  readonly name: string;
  readonly valueType: ScalarValueType | null;
  readonly create: unknown;
  readonly reference: unknown;
  readonly [COMPLETE_BRAND]?: string;
  readonly [REFERENCE_BRAND]?: string;
  readonly [HYDRATE_COMPLETE_BRAND]: (iid: string | null, input: unknown) => unknown;
}

interface RuntimeModelDefinition {
  readonly valueType: ScalarValueType | null;
  readonly createMembers: readonly CreateMemberDefinition[];
  readonly completeMembers: readonly CreateMemberDefinition[];
  readonly referenceKeys: readonly string[];
  readonly metadata: unknown;
}

interface RuntimeModelEntry {
  readonly token: RuntimeModelToken;
  readonly definition: RuntimeModelDefinition;
}

interface ScalarWire {
  readonly valueType: ScalarValueType;
  readonly value: string | number | boolean;
}

interface ProjectedWire {
  readonly typeKey: string;
  readonly form: ProjectedModelForm;
  readonly iid: string | null;
  readonly value: ScalarWire | null;
  readonly values: Readonly<Record<string, unknown>>;
}

const runtimeModels = new Map<string, RuntimeModelEntry>();
const fieldTokenStates = new WeakMap<object, FieldTokenDefinition<string, string>>();
const roleTokenStates = new WeakMap<object, RoleTokenDefinition<string, string>>();
let installedProjection: InstalledRuntimeProjection | null = null;
let installedQueryAuthority: QueryV2Authority | null = null;

interface StructFieldDefinition {
  readonly name: string;
  readonly optional: boolean;
}

function assertRecord(value: unknown, context: string): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${context} must be an object`);
  }
}

function validateMultiplicity(name: string, value: unknown, multiplicity: Multiplicity): void {
  if (multiplicity.container === "sequence" && value !== undefined && value !== null && !Array.isArray(value)) {
    throw new TypeError(`${name} must be a sequence`);
  }
  if (multiplicity.container === "scalar" && Array.isArray(value)) {
    throw new TypeError(`${name} must be scalar`);
  }
  const count = value === undefined || value === null ? 0 : Array.isArray(value) ? value.length : 1;
  const actual = BigInt(count);
  const minimum = BigInt(multiplicity.cardinality.min);
  const maximum = multiplicity.cardinality.max === null ? null : BigInt(multiplicity.cardinality.max);
  if (actual < minimum || (maximum !== null && actual > maximum)) {
    throw new RangeError(`${name} violates projected cardinality`);
  }
}

function validateAccepted(name: string, value: unknown, accepted: readonly ModelUse[]): void {
  if (value === undefined || value === null) {
    return;
  }
  const values: readonly unknown[] = Array.isArray(value) ? value : [value];
  for (const candidate of values) {
    assertRecord(candidate, name);
    const matches = accepted.some(
      (model) => candidate["__typebridgeModel"] === model.typeKey
        && candidate["__typebridgeForm"] === model.form,
    );
    if (!matches) {
      throw new TypeError(`${name} has an incompatible projected model form`);
    }
  }
}

function validateScalar(name: string, value: unknown, valueType: ScalarValueType): void {
  const valid = (() => {
    switch (valueType) {
      case "string":
      case "decimal":
      case "duration":
        return typeof value === "string";
      case "long":
        return typeof value === "bigint"
          && value >= -(1n << 63n)
          && value <= (1n << 63n) - 1n;
      case "double":
        return typeof value === "number" && Number.isFinite(value);
      case "boolean":
        return typeof value === "boolean";
      case "date":
      case "datetime":
      case "datetime_tz":
        return value instanceof Date && Number.isFinite(value.getTime());
    }
  })();
  if (!valid) {
    throw new TypeError(`${name} must be a canonical ${valueType} scalar`);
  }
}

function freezeValue(value: unknown): unknown {
  return Array.isArray(value) ? Object.freeze([...value]) : value;
}

export function defineFieldToken<Owner extends string, Attribute extends string, Value>(
  definition: FieldTokenDefinition<Owner, Attribute>,
): FieldToken<Owner, Attribute, Value> {
  const token = { kind: "field", ...definition } as FieldToken<Owner, Attribute, Value>;
  fieldTokenStates.set(token, definition);
  return Object.freeze(token);
}

export function defineRoleToken<
  Owner extends string,
  Role extends string,
  Player,
  SubtypeRoot = Player,
>(
  definition: RoleTokenDefinition<Owner, Role>,
): RoleToken<Owner, Role, Player, SubtypeRoot> {
  const token = { kind: "role", ...definition } as RoleToken<
    Owner,
    Role,
    Player,
    SubtypeRoot
  >;
  roleTokenStates.set(token, definition);
  return Object.freeze(token);
}

export function defineModel<
  Id extends string,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields extends object,
  Roles extends object,
>(definition: ModelDefinition<Id, Fields, Roles>): ModelToken<
  Id,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields,
  Roles
> {
  const materializeComplete = (
    input: unknown,
    iid: string | null,
    members: readonly CreateMemberDefinition[],
    context: string,
  ): Complete => {
    if (iid !== null && (typeof iid !== "string" || iid.length === 0)) {
      throw new TypeError(`${context} iid must be a non-empty string`);
    }
    const result: Record<string | symbol, unknown> = {
      __typebridgeModel: definition.typeKey,
      __typebridgeForm: "complete",
      iid,
    };
    if (definition.valueType !== null) {
      validateScalar(`${context} value`, input, definition.valueType);
      requireProjection().validateAttributeValueJson(
        definition.typeKey,
        JSON.stringify(scalarToWire(definition.valueType, input)),
      );
      result["value"] = freezeValue(input);
    } else {
      assertRecord(input, `${context} input`);
      const allowed = new Map(members.map((member) => [member.name, member]));
        for (const name of Object.keys(input)) {
          if (!allowed.has(name)) {
            throw new TypeError(`${context} received unknown member ${name}`);
          }
        }
        for (const member of members) {
          const value = input[member.name];
          validateMultiplicity(member.name, value, member.multiplicity);
          if (member.accepted !== undefined) {
            validateAccepted(member.name, value, member.accepted);
          }
          if (Object.hasOwn(definition.fields, member.name)) {
            validateOwnedMember(definition.typeKey, member.name, value);
          }
          result[member.name] = value === undefined || value === null
            ? member.multiplicity.container === "sequence" ? Object.freeze([]) : null
            : freezeValue(value);
        }
    }
    Object.defineProperty(result, COMPLETE_BRAND, { value: definition.typeKey });
    return Object.freeze(result) as Complete;
  };
  const create = definition.createEnabled
    ? (input: unknown): Complete => materializeComplete(
        input,
        null,
        definition.createMembers,
        `${definition.name}.create`,
      )
    : undefined;
  const reference = definition.referenceEnabled
    ? (iid: string, keys: unknown): unknown => {
        if (typeof iid !== "string" || iid.length === 0) {
          throw new TypeError(`${definition.name}.reference iid must be a non-empty string`);
        }
        assertRecord(keys, `${definition.name}.reference keys`);
        const allowed = new Set(definition.referenceKeys);
        for (const name of Object.keys(keys)) {
          if (!allowed.has(name)) {
            throw new TypeError(`${definition.name}.reference received unknown key ${name}`);
          }
        }
        for (const name of definition.referenceKeys) {
          if (!(name in keys)) {
            throw new TypeError(`${definition.name}.reference is missing key ${name}`);
          }
        }
        const result: Record<string | symbol, unknown> = {
          __typebridgeModel: definition.typeKey,
          __typebridgeForm: "reference",
          iid,
          ...keys,
        };
        Object.defineProperty(result, REFERENCE_BRAND, { value: definition.typeKey });
        return Object.freeze(result);
      }
    : undefined;
  const token = Object.assign(
    {
      kind: "type" as const,
      typeKey: definition.typeKey,
      id: definition.id,
      name: definition.name,
      valueType: definition.valueType,
      fields: Object.freeze(definition.fields),
      roles: Object.freeze(definition.roles),
      create,
      reference,
      metadata: definition.metadata,
    },
    definition.fields,
    definition.roles,
  );
  Object.defineProperty(token, TYPE_TOKEN_BRAND, { value: definition.typeKey });
  Object.defineProperty(token, HYDRATE_COMPLETE_BRAND, {
    value: (iid: string | null, input: unknown): Complete => materializeComplete(
      input,
      iid,
      definition.completeMembers,
      `${definition.name}.hydrate`,
    ),
  });
  Object.defineProperty(token, "manager", {
    enumerable: true,
    value: (connection: RuntimeProjectionConnection): ProjectedModelManager<Complete> => projectedManager(
      definition.typeKey,
      connection,
    ),
  });
  const modelToken = token as ModelToken<
    Id,
    Complete,
    CreateFactory,
    ReferenceFactory,
    Fields,
    Roles
  >;
  if (runtimeModels.has(definition.typeKey)) {
    throw new TypeError(`duplicate generated model token ${definition.typeKey}`);
  }
  runtimeModels.set(definition.typeKey, { token: modelToken, definition });
  Object.freeze(modelToken);
  return modelToken;
}

function validateOwnedMember(typeKey: string, name: string, value: unknown): void {
  if (value === undefined || value === null) {
    return;
  }
  const values: readonly unknown[] = Array.isArray(value) ? value : [value];
  for (const candidate of values) {
    assertRecord(candidate, name);
    const attributeTypeKey = candidate["__typebridgeModel"];
    if (typeof attributeTypeKey !== "string") {
      throw new TypeError(`${name} has no projected attribute identity`);
    }
    const attribute = runtimeModels.get(attributeTypeKey);
    if (attribute === undefined || attribute.definition.valueType === null) {
      throw new TypeError(`${name} is not an exact projected attribute value`);
    }
    requireProjection().validateFieldValueJson(
      typeKey,
      name,
      JSON.stringify(scalarToWire(attribute.definition.valueType, candidate["value"])),
    );
  }
}

/** @internal Install exact native evidence after every generated model token is linked. */
export function __installRuntimeProjectionPackage(
  projectionJson: string,
  semanticFingerprintJson: string,
  projectionFingerprintJson: string,
  tokens: readonly object[],
  schemaAuthorityJson: string,
): void {
  if (installedProjection !== null) {
    throw new TypeError("generated runtime projection is already installed");
  }
  const entries = tokens.map((token) => {
    const entry = [...runtimeModels.values()].find((candidate) => candidate.token === token);
    if (entry === undefined) {
      throw new TypeError("runtime projection registration contains an unknown model token");
    }
    return entry;
  });
  const authority = installGeneratedSchemaAuthority({
    schemaAuthorityJson,
    semanticFingerprintJson,
  });
  const projection = installRuntimeProjection({
    projectionJson,
    semanticFingerprintJson,
    projectionFingerprintJson,
    bindings: entries.map(({ token }) => ({
      typeKey: token.typeKey,
      targetName: token.name,
      create: typeof token.create === "function",
      reference: typeof token.reference === "function",
    })),
  });
  installedProjection = projection;
  installedQueryAuthority = authority;
}

function projectedManager<Complete>(
  typeKey: string,
  connection: RuntimeProjectionConnection,
): ProjectedModelManager<Complete> {
  const projection = installedProjection;
  if (projection === null) {
    throw new TypeError("generated runtime projection is not installed");
  }
  return projectedManagerForNative(typeKey, projection.manager(typeKey, connection));
}

function projectedManagerForNative<Complete>(
  typeKey: string,
  native: NativeProjectedManager,
): ProjectedModelManager<Complete> {
  return Object.freeze({
    insert(instance: Complete): Complete {
      const wire = lowerProjectedValue(instance);
      if (wire.typeKey !== typeKey || wire.form !== "complete") {
        throw new TypeError("projected insert requires the manager's exact complete model");
      }
      return hydrateNativeResult(native.insertJson(JSON.stringify(wire)), typeKey);
    },
    insertMany(instances: readonly Complete[]): readonly Complete[] {
      const wires = lowerManagerBatch(instances, typeKey, "insertMany");
      return hydrateNativeResults(native.insertManyJson(JSON.stringify(wires)), typeKey);
    },
    put(instance: Complete): Complete {
      const wire = lowerProjectedValue(instance);
      if (wire.typeKey !== typeKey || wire.form !== "complete") {
        throw new TypeError("projected put requires the manager's exact complete model");
      }
      return hydrateNativeResult(native.putJson(JSON.stringify(wire)), typeKey);
    },
    putMany(instances: readonly Complete[]): readonly Complete[] {
      const wires = lowerManagerBatch(instances, typeKey, "putMany");
      return hydrateNativeResults(native.putManyJson(JSON.stringify(wires)), typeKey);
    },
    update(iid: string, replacement: Complete): Complete {
      if (typeof iid !== "string" || iid.length === 0) {
        throw new TypeError("projected manager update requires a non-empty TypeDB IID");
      }
      const wire = lowerProjectedValue(replacement);
      if (wire.typeKey !== typeKey || wire.form !== "complete") {
        throw new TypeError("projected update requires the manager's exact complete model");
      }
      return hydrateNativeResult(native.updateJson(iid, JSON.stringify(wire)), typeKey);
    },
    delete(instanceOrIid: Complete | string): void {
      if (typeof instanceOrIid === "string") {
        native.deleteByIid(instanceOrIid);
        return;
      }
      const wire = lowerProjectedValue(instanceOrIid);
      if (wire.typeKey !== typeKey || wire.form !== "complete") {
        throw new TypeError("projected delete requires the manager's exact complete model");
      }
      if (wire.iid === null) {
        throw new TypeError("projected manager delete requires an attached TypeDB IID");
      }
      native.deleteByIid(wire.iid);
    },
    filter(
      filters: Readonly<Record<string, ProjectedManagerFilterValue>>,
    ): ProjectedModelManager<Complete> {
      if (filters === null || typeof filters !== "object" || Array.isArray(filters)) {
        throw new TypeError("projected manager filters require a string-keyed object");
      }
      const lowered: Record<string, unknown> = {};
      for (const [name, value] of Object.entries(filters)) {
        lowered[name] = lowerManagerFilterValue(value);
      }
      return projectedManagerForNative(typeKey, native.filterJson(JSON.stringify(lowered)));
    },
    getByIid(iid: string): Complete | null {
      const value = JSON.parse(native.getByIidJson(iid)) as unknown;
      if (value === null) {
        return null;
      }
      return hydrateProjectedValue(parseProjectedWire(value), typeKey) as Complete;
    },
    all(): readonly Complete[] {
      return hydrateNativeResults(native.allJson(), typeKey);
    },
    first(): Complete | null {
      const value = JSON.parse(native.firstJson()) as unknown;
      if (value === null) {
        return null;
      }
      return hydrateProjectedValue(parseProjectedWire(value), typeKey) as Complete;
    },
    count(): bigint {
      return native.count();
    },
    exists(): boolean {
      return native.exists();
    },
  });
}

function lowerManagerFilterValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(lowerManagerFilterValue);
  }
  if (isProjectedModelValue(value)) {
    return lowerProjectedValue(value);
  }
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  throw new TypeError(
    "projected manager filter values must be finite JSON scalars, generated model values, or arrays of those values",
  );
}

function lowerManagerBatch<Complete>(
  instances: readonly Complete[],
  typeKey: string,
  operation: string,
): readonly ProjectedWire[] {
  if (!Array.isArray(instances)) {
    throw new TypeError(`projected manager ${operation} requires an array`);
  }
  return instances.map((instance) => {
    const wire = lowerProjectedValue(instance);
    if (wire.typeKey !== typeKey || wire.form !== "complete") {
      throw new TypeError(`projected ${operation} requires the manager's exact complete model`);
    }
    return wire;
  });
}

function isProjectedModelValue(value: unknown): value is object {
  return typeof value === "object" && value !== null && "__typebridgeModel" in value;
}

function hydrateNativeResult<Complete>(json: string, expectedTypeKey: string): Complete {
  const value = JSON.parse(json) as unknown;
  return hydrateProjectedValue(parseProjectedWire(value), expectedTypeKey) as Complete;
}

function hydrateNativeResults<Complete>(
  json: string,
  expectedTypeKey: string,
): readonly Complete[] {
  const values = JSON.parse(json) as unknown;
  if (!Array.isArray(values)) {
    throw new TypeError("native projected manager returned a non-sequence result");
  }
  return Object.freeze(values.map(
    (value) => hydrateProjectedValue(parseProjectedWire(value), expectedTypeKey) as Complete,
  ));
}

function lowerProjectedValue(value: unknown): ProjectedWire {
  assertRecord(value, "projected value");
  const typeKey = value["__typebridgeModel"];
  const form = value["__typebridgeForm"];
  if (typeof typeKey !== "string" || (form !== "complete" && form !== "reference")) {
    throw new TypeError("value is not a generated projected model");
  }
  const entry = runtimeModels.get(typeKey);
  if (entry === undefined) {
    throw new TypeError("value belongs to a different generated runtime projection");
  }
  if (form === "complete" && Object.getOwnPropertyDescriptor(value, COMPLETE_BRAND)?.value !== typeKey) {
    throw new TypeError("complete value does not carry the generated nominal brand");
  }
  if (form === "reference" && Object.getOwnPropertyDescriptor(value, REFERENCE_BRAND)?.value !== typeKey) {
    throw new TypeError("reference value does not carry the generated nominal brand");
  }
  const iid = value["iid"];
  if (iid !== null && (typeof iid !== "string" || iid.length === 0)) {
    throw new TypeError("projected IID must be null or a non-empty string");
  }
  if (entry.definition.valueType !== null) {
    if (form !== "complete") {
      throw new TypeError("attribute values cannot use a reference projection");
    }
    return {
      typeKey,
      form,
      iid,
      value: scalarToWire(entry.definition.valueType, value["value"]),
      values: {},
    };
  }
  const members = form === "complete"
    ? entry.definition.createMembers
    : entry.definition.referenceKeys.map((name) => ({
        name,
        multiplicity: { cardinality: { kind: "cardinality", min: "1", max: "1" }, required: true, container: "scalar" },
      }));
  const values: Record<string, unknown> = {};
  for (const member of members) {
    values[member.name] = lowerMemberValue(value[member.name]);
  }
  return { typeKey, form, iid, value: null, values };
}

function lowerMemberValue(value: unknown): unknown {
  if (value === null || value === undefined) {
    return null;
  }
  if (Array.isArray(value)) {
    return value.map(lowerProjectedValue);
  }
  return lowerProjectedValue(value);
}

function scalarToWire(valueType: ScalarValueType, value: unknown): ScalarWire {
  validateScalar("projected attribute value", value, valueType);
  switch (valueType) {
    case "long":
      return { valueType, value: (value as bigint).toString() };
    case "date": {
      const iso = (value as Date).toISOString();
      if (!iso.endsWith("T00:00:00.000Z")) {
        throw new TypeError("date attributes require UTC midnight");
      }
      return { valueType, value: iso.slice(0, 10) };
    }
    case "datetime":
      return { valueType, value: canonicalDateTime((value as Date).toISOString(), false) };
    case "datetime_tz":
      return { valueType, value: canonicalDateTime((value as Date).toISOString(), true) };
    default:
      return { valueType, value: value as string | number | boolean };
  }
}

function canonicalDateTime(iso: string, timezone: boolean): string {
  const withoutZeroFraction = iso.replace(".000Z", "Z");
  return timezone ? withoutZeroFraction : withoutZeroFraction.slice(0, -1);
}

function parseProjectedWire(value: unknown): ProjectedWire {
  assertRecord(value, "native projected wire");
  const typeKey = value["typeKey"];
  const form = value["form"];
  const iid = value["iid"];
  const scalar = value["value"];
  const values = value["values"];
  if (typeof typeKey !== "string" || (form !== "complete" && form !== "reference")) {
    throw new TypeError("native projected wire has an invalid model identity or form");
  }
  if (iid !== null && (typeof iid !== "string" || iid.length === 0)) {
    throw new TypeError("native projected wire has an invalid IID");
  }
  assertRecord(values, "native projected wire values");
  let scalarWire: ScalarWire | null = null;
  if (scalar !== null) {
    assertRecord(scalar, "native scalar wire");
    const valueType = scalar["valueType"];
    if (!isScalarValueType(valueType)) {
      throw new TypeError("native scalar wire has an invalid value type");
    }
    const scalarValue = scalar["value"];
    if (typeof scalarValue !== "string" && typeof scalarValue !== "number" && typeof scalarValue !== "boolean") {
      throw new TypeError("native scalar wire has an invalid value");
    }
    scalarWire = { valueType, value: scalarValue };
  }
  return { typeKey, form, iid, value: scalarWire, values };
}

function hydrateProjectedValue(wire: ProjectedWire, expectedTypeKey?: string): unknown {
  if (expectedTypeKey !== undefined && wire.typeKey !== expectedTypeKey) {
    throw new TypeError("native projected wire returned a different concrete type");
  }
  const entry = runtimeModels.get(wire.typeKey);
  if (entry === undefined) {
    throw new TypeError("native projected wire references an unregistered model");
  }
  if (entry.definition.valueType !== null) {
    if (wire.form !== "complete" || wire.value === null || Object.keys(wire.values).length !== 0) {
      throw new TypeError("native attribute wire has an invalid shape");
    }
    return entry.token[HYDRATE_COMPLETE_BRAND](null, scalarFromWire(wire.value));
  }
  const names = wire.form === "complete"
    ? entry.definition.completeMembers.map((member) => member.name)
    : entry.definition.referenceKeys;
  const values: Record<string, unknown> = {};
  for (const name of names) {
    if (!(name in wire.values)) {
      throw new TypeError(`native projected wire is missing member ${name}`);
    }
    values[name] = hydrateMemberValue(wire.values[name]);
  }
  if (wire.form === "complete") {
    return entry.token[HYDRATE_COMPLETE_BRAND](wire.iid, values);
  }
  if (wire.iid === null || typeof entry.token.reference !== "function") {
    throw new TypeError("native reference wire has no IID or reference factory");
  }
  return entry.token.reference(wire.iid, values);
}

function hydrateMemberValue(value: unknown): unknown {
  if (value === null) {
    return null;
  }
  if (Array.isArray(value)) {
    return Object.freeze(value.map((item) => hydrateProjectedValue(parseProjectedWire(item))));
  }
  return hydrateProjectedValue(parseProjectedWire(value));
}

function scalarFromWire(wire: ScalarWire): unknown {
  switch (wire.valueType) {
    case "long":
      if (typeof wire.value !== "string") throw new TypeError("long wire requires a string");
      return BigInt(wire.value);
    case "date":
      if (typeof wire.value !== "string") throw new TypeError("date wire requires a string");
      return new Date(`${wire.value}T00:00:00.000Z`);
    case "datetime":
      if (typeof wire.value !== "string") throw new TypeError("datetime wire requires a string");
      return new Date(`${wire.value}Z`);
    case "datetime_tz":
      if (typeof wire.value !== "string") throw new TypeError("datetime-tz wire requires a string");
      return new Date(wire.value);
    default:
      return wire.value;
  }
}

function isScalarValueType(value: unknown): value is ScalarValueType {
  return typeof value === "string" && [
    "string", "long", "double", "boolean", "date", "datetime", "datetime_tz", "decimal", "duration",
  ].includes(value);
}

declare const SELECTION_BRAND: unique symbol;
declare const BOUND_VAR_BRAND: unique symbol;
declare const BOUND_FIELD_BRAND: unique symbol;
declare const BOUND_ROLE_BRAND: unique symbol;
declare const PREDICATE_BRAND: unique symbol;
declare const ORDER_BRAND: unique symbol;
declare const AGGREGATE_BRAND: unique symbol;

/** A generated model token accepted by the package-local query facade. */
export type QueryModelToken<Id extends string, Complete extends object> = TypeToken<Id> & {
  readonly __complete?: Complete;
};

/** Covariant output-only view of one generated query selection. */
export interface Selection<out Output> {
  readonly [SELECTION_BRAND]: () => Output;
}

/** One immutable package-local boolean predicate. */
export interface Predicate {
  readonly [PREDICATE_BRAND]: true;
  and(other: Predicate): Predicate;
  or(other: Predicate): Predicate;
  not(): Predicate;
}

/** One immutable package-local ordering term. */
export interface QueryOrder {
  readonly [ORDER_BRAND]: true;
}

/** One immutable typed reducer term over a query's distinct-root stream. */
export interface Aggregate<out Output> {
  readonly [AGGREGATE_BRAND]: () => Output;
}

type ModelTypeKey<Model extends object> = Model extends CompleteFacet<infer Id> ? Id : never;
type QueryMatchMode = "exact" | "subtypes";
type RoleBindingCompatibility<
  Actual extends object,
  Mode extends QueryMatchMode,
  Player extends object,
  SubtypeRoot extends object,
> = Mode extends "subtypes"
  ? Actual extends SubtypeRoot ? object : never
  : Actual extends Player ? object : never;

/** One generated field token bound to one exact generated model variable. */
export interface BoundField<Value> {
  readonly [BOUND_FIELD_BRAND]: (value: Value) => Value;
  eq(value: Value | BoundField<Value>): Predicate;
  eqField(field: BoundField<Value>): Predicate;
  ne(value: Value | BoundField<Value>): Predicate;
  gt(value: Value | BoundField<Value>): Predicate;
  gte(value: Value | BoundField<Value>): Predicate;
  lt(value: Value | BoundField<Value>): Predicate;
  lte(value: Value | BoundField<Value>): Predicate;
  contains(value: Value): Predicate;
  startsWith(value: Value): Predicate;
  endsWith(value: Value): Predicate;
  regex(value: Value): Predicate;
  isPresent(): Predicate;
  isMissing(): Predicate;
  asc(missing?: "reject" | "first" | "last"): QueryOrder;
  desc(missing?: "reject" | "first" | "last"): QueryOrder;
}

/** One generated role token bound to one exact generated relation variable. */
export interface BoundRole<
  in out Player extends object,
  in out SubtypeRoot extends object = Player,
> {
  readonly [BOUND_ROLE_BRAND]: (
    player: Player,
    subtypeRoot: SubtypeRoot,
  ) => readonly [Player, SubtypeRoot];
  connects<Actual extends object, Mode extends QueryMatchMode>(
    player: BoundVar<Actual, Mode>
      & RoleBindingCompatibility<Actual, Mode, Player, SubtypeRoot>,
  ): Predicate;
  is<Actual extends object, Mode extends QueryMatchMode>(
    player: BoundVar<Actual, Mode>
      & RoleBindingCompatibility<Actual, Mode, Player, SubtypeRoot>,
  ): Predicate;
}

/** A singular generated model selection. */
export interface BoundVar<
  in out Model extends object,
  out Mode extends QueryMatchMode = QueryMatchMode,
> extends Selection<Model> {
  readonly [BOUND_VAR_BRAND]: (model: Model) => readonly [Model, Mode];
  field<Owner extends string, Attribute extends string, Value>(
    token: FieldToken<Owner, Attribute, Value>
      & (Owner extends ModelTypeKey<Model> ? object : never),
  ): BoundField<Value>;
  role<
    Owner extends string,
    Role extends string,
    Player extends object,
    SubtypeRoot extends object,
  >(
    token: RoleToken<Owner, Role, Player, SubtypeRoot>
      & (Owner extends ModelTypeKey<Model> ? object : never),
  ): BoundRole<Player, SubtypeRoot>;
  iid(iid: string): Predicate;
  iidIn(iids: readonly string[]): Predicate;
  collect(): Collected<Model>;
}

/** A persistent collection selection over one generated model variable. */
export interface Collected<in out Model extends object> extends Selection<readonly Model[]> {
  distinct(distinct?: boolean): Collected<Model>;
  orderBy(order: QueryOrder): Collected<Model>;
}

export interface RowsOptions {
  readonly orderBy?: readonly QueryOrder[];
  readonly offset?: bigint;
  readonly limit: bigint;
}

export interface PageOptions extends RowsOptions {
  readonly includeTotal?: boolean;
}

/** Immutable distinct-root query page. */
export interface Page<out Item> {
  readonly items: readonly Item[];
  readonly offset: bigint;
  readonly limit: bigint;
  readonly total: bigint | null;
}

export interface FirstOptions {
  readonly orderBy?: readonly QueryOrder[];
}

type AttributeScalar<Value> = Value extends { readonly value: infer Scalar } ? Scalar : never;
type NumericField<Value> = AttributeScalar<Value> extends bigint | number
  ? BoundField<Value>
  : never;
type NumericOutput<Value> = Extract<AttributeScalar<Value>, bigint | number>;
type AggregateOutput<Term> = Term extends Aggregate<infer Output> ? Output : never;
export type AggregateOutputs<Terms extends readonly Aggregate<unknown>[]> = Readonly<{
  [Index in keyof Terms]: AggregateOutput<Terms[Index]>;
}>;

type SelectionOutput<Value> = Value extends Selection<infer Output> ? Output : never;
type PositionalOutput<Selections extends readonly Selection<unknown>[]> =
  Selections extends readonly [Selection<infer Only>] ? Only : {
    readonly [Index in keyof Selections]: SelectionOutput<Selections[Index]>;
  };
type NamedOutput<Shape extends Readonly<Record<string, Selection<unknown>>>> = Readonly<{
  [Key in keyof Shape]: SelectionOutput<Shape[Key]>;
}>;
type AtMostSixteen<Values extends readonly unknown[]> = Values extends readonly [
  unknown, unknown, unknown, unknown, unknown, unknown, unknown, unknown, unknown,
  unknown, unknown, unknown, unknown, unknown, unknown, unknown, unknown, ...unknown[],
] ? never : Values;
type QuerySlotCount = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16;
type UnionToIntersection<Union> = (
  Union extends unknown ? (value: Union) => void : never
) extends (value: infer Intersection) => void ? Intersection : never;
type LastUnionMember<Union> = UnionToIntersection<
  Union extends unknown ? () => Union : never
> extends () => infer Last ? Last : never;
type UnionToTuple<Union, Last = LastUnionMember<Union>> = [Union] extends [never]
  ? []
  : [...UnionToTuple<Exclude<Union, Last>>, Last];
type NamedSelectionInput<Shape extends Readonly<Record<string, Selection<unknown>>>> =
  keyof Shape extends never
    ? never
    : string extends keyof Shape
      ? never
      : Exclude<keyof Shape, string> extends never
        ? UnionToTuple<Extract<keyof Shape, string>>["length"] extends QuerySlotCount
          ? Shape
          : never
        : never;

type NativeRole = ReturnType<RuntimeProjectionMatchBinding["roleOwnedBy"]>;
interface SelectionState {
  readonly handle: RuntimeProjectionMatchSelection;
  readonly projection: InstalledRuntimeProjection;
  readonly modelTypeKey: string;
  readonly collection: boolean;
}
interface BoundVarState extends SelectionState {
  readonly binding: RuntimeProjectionMatchBinding;
  readonly matchMode: QueryMatchMode;
}
interface BoundFieldState {
  readonly handle: RuntimeProjectionMatchField;
  readonly projection: InstalledRuntimeProjection;
  readonly attributeTypeKey: string;
}
interface BoundRoleState {
  readonly handle: NativeRole;
  readonly projection: InstalledRuntimeProjection;
  readonly acceptedPlayers: ReadonlySet<string>;
}

const selectionStates = new WeakMap<object, SelectionState>();
const boundVarStates = new WeakMap<object, BoundVarState>();
const boundFieldQueryStates = new WeakMap<object, BoundFieldState>();
const boundRoleQueryStates = new WeakMap<object, BoundRoleState>();
const predicateQueryStates = new WeakMap<object, Readonly<{
  handle: RuntimeProjectionMatchPredicate;
  projection: InstalledRuntimeProjection;
}>>();
const orderQueryStates = new WeakMap<object, Readonly<{
  handle: RuntimeProjectionMatchOrder;
  projection: InstalledRuntimeProjection;
}>>();

interface ReductionOutputSpec {
  readonly kind: "count" | "long" | "double";
  readonly optional: boolean;
}

interface AggregateState {
  readonly reducer: RuntimeProjectionReduction;
  readonly input: RuntimeProjectionMatchField | null;
  readonly projection: InstalledRuntimeProjection | null;
  readonly output: ReductionOutputSpec;
}

const aggregateQueryStates = new WeakMap<object, AggregateState>();

class AggregateValue<Output> implements Aggregate<Output> {
  declare readonly [AGGREGATE_BRAND]: () => Output;

  constructor(state: AggregateState) {
    aggregateQueryStates.set(this, Object.freeze(state));
    Object.freeze(this);
  }
}

function fieldAggregate<Value, Output>(
  reducer: Exclude<RuntimeProjectionReduction, "count">,
  field: NumericField<Value>,
  optional: boolean,
  forceDouble = false,
): Aggregate<Output> {
  const state = boundFieldState(field);
  const valueType = runtimeModels.get(state.attributeTypeKey)?.definition.valueType;
  if (valueType !== "long" && valueType !== "double") {
    throw new TypeError("generated reductions require a long or double field");
  }
  return new AggregateValue<Output>({
    reducer,
    input: state.handle,
    projection: state.projection,
    output: Object.freeze({
      kind: forceDouble || valueType === "double" ? "double" : "long",
      optional,
    }),
  });
}

/** Canonical generated reducer constructors. */
export const aggregate = Object.freeze({
  count(): Aggregate<bigint> {
    return new AggregateValue<bigint>({
      reducer: "count",
      input: null,
      projection: null,
      output: Object.freeze({ kind: "count", optional: false }),
    });
  },
  sum<Value>(field: NumericField<Value>): Aggregate<NumericOutput<Value>> {
    return fieldAggregate<Value, NumericOutput<Value>>("sum", field, false);
  },
  min<Value>(field: NumericField<Value>): Aggregate<NumericOutput<Value> | null> {
    return fieldAggregate<Value, NumericOutput<Value> | null>("min", field, true);
  },
  max<Value>(field: NumericField<Value>): Aggregate<NumericOutput<Value> | null> {
    return fieldAggregate<Value, NumericOutput<Value> | null>("max", field, true);
  },
  mean<Value>(field: NumericField<Value>): Aggregate<number | null> {
    return fieldAggregate<Value, number | null>("mean", field, true, true);
  },
  median<Value>(field: NumericField<Value>): Aggregate<number | null> {
    return fieldAggregate<Value, number | null>("median", field, true, true);
  },
  std<Value>(field: NumericField<Value>): Aggregate<number | null> {
    return fieldAggregate<Value, number | null>("std", field, true, true);
  },
});

function requireProjection(): InstalledRuntimeProjection {
  if (installedProjection === null) {
    throw new TypeError("generated runtime projection is not installed");
  }
  return installedProjection;
}

function requireQueryAuthority(): QueryV2Authority {
  if (installedQueryAuthority === null) {
    throw new TypeError("generated schema authority is not installed");
  }
  return installedQueryAuthority;
}

function exactModelToken(token: object): RuntimeModelEntry {
  const entry = [...runtimeModels.values()].find((candidate) => candidate.token === token);
  if (entry === undefined) {
    throw new TypeError("generated query requires an exact package model token");
  }
  return entry;
}

function requireSameProjection(
  expected: InstalledRuntimeProjection,
  actual: InstalledRuntimeProjection,
  context: string,
): void {
  if (actual !== expected) {
    throw new TypeError(`${context} belongs to another generated package projection`);
  }
}

class PredicateValue implements Predicate {
  declare readonly [PREDICATE_BRAND]: true;

  constructor(handle: RuntimeProjectionMatchPredicate, projection: InstalledRuntimeProjection) {
    predicateQueryStates.set(this, { handle, projection });
    Object.freeze(this);
  }

  and(other: Predicate): Predicate {
    const own = predicateState(this);
    const right = predicateState(other);
    requireSameProjection(own.projection, right.projection, "generated predicate");
    return new PredicateValue(own.handle.and(right.handle), own.projection);
  }

  or(other: Predicate): Predicate {
    const own = predicateState(this);
    const right = predicateState(other);
    requireSameProjection(own.projection, right.projection, "generated predicate");
    return new PredicateValue(own.handle.or(right.handle), own.projection);
  }

  not(): Predicate {
    const own = predicateState(this);
    return new PredicateValue(own.handle.not(), own.projection);
  }
}

class QueryOrderValue implements QueryOrder {
  declare readonly [ORDER_BRAND]: true;

  constructor(handle: RuntimeProjectionMatchOrder, projection: InstalledRuntimeProjection) {
    orderQueryStates.set(this, { handle, projection });
    Object.freeze(this);
  }
}

class BoundFieldValue<Value> implements BoundField<Value> {
  declare readonly [BOUND_FIELD_BRAND]: (value: Value) => Value;

  constructor(state: BoundFieldState) {
    boundFieldQueryStates.set(this, state);
    Object.freeze(this);
  }

  eq(value: Value | BoundField<Value>): Predicate { return this.#compare("equal", value); }
  eqField(field: BoundField<Value>): Predicate { return this.#compare("equal", field); }
  ne(value: Value | BoundField<Value>): Predicate { return this.#compare("not_equal", value); }
  gt(value: Value | BoundField<Value>): Predicate { return this.#compare("greater_than", value); }
  gte(value: Value | BoundField<Value>): Predicate { return this.#compare("greater_than_or_equal", value); }
  lt(value: Value | BoundField<Value>): Predicate { return this.#compare("less_than", value); }
  lte(value: Value | BoundField<Value>): Predicate { return this.#compare("less_than_or_equal", value); }
  contains(value: Value): Predicate { return this.#compare("contains", value); }
  startsWith(value: Value): Predicate { return this.#compare("starts_with", value); }
  endsWith(value: Value): Predicate { return this.#compare("ends_with", value); }
  regex(value: Value): Predicate { return this.#compare("regex", value); }
  isPresent(): Predicate { return this.#presence(true); }
  isMissing(): Predicate { return this.#presence(false); }

  asc(missing: "reject" | "first" | "last" = "reject"): QueryOrder {
    return this.#order("ascending", missing);
  }

  desc(missing: "reject" | "first" | "last" = "reject"): QueryOrder {
    return this.#order("descending", missing);
  }

  #compare(
    operator:
      | "equal"
      | "not_equal"
      | "greater_than"
      | "greater_than_or_equal"
      | "less_than"
      | "less_than_or_equal"
      | "contains"
      | "starts_with"
      | "ends_with"
      | "regex",
    value: Value | BoundField<Value>,
  ): Predicate {
    const own = boundFieldState(this);
    const other = boundFieldQueryStates.get(value as object);
    if (other !== undefined) {
      requireSameProjection(own.projection, other.projection, "generated field");
      if (other.attributeTypeKey !== own.attributeTypeKey) {
        throw new TypeError("generated field comparisons require the same attribute type");
      }
      return new PredicateValue(own.handle.compareField(operator, other.handle), own.projection);
    }
    const wire = lowerProjectedValue(value);
    if (wire.typeKey !== own.attributeTypeKey || wire.form !== "complete" || wire.value === null) {
      throw new TypeError("generated field comparison requires its exact attribute wrapper");
    }
    const dynamic = JSON.stringify({ value_type: wire.value.valueType, value: wire.value.value });
    return new PredicateValue(own.handle.compareValueJson(operator, dynamic), own.projection);
  }

  #order(
    direction: "ascending" | "descending",
    missing: "reject" | "first" | "last",
  ): QueryOrder {
    const own = boundFieldState(this);
    return new QueryOrderValue(own.handle.order(direction, missing), own.projection);
  }

  #presence(present: boolean): Predicate {
    const own = boundFieldState(this);
    return new PredicateValue(own.handle.presence(present), own.projection);
  }
}

class BoundRoleValue<Player extends object, SubtypeRoot extends object>
  implements BoundRole<Player, SubtypeRoot> {
  declare readonly [BOUND_ROLE_BRAND]: (
    player: Player,
    subtypeRoot: SubtypeRoot,
  ) => readonly [Player, SubtypeRoot];

  constructor(state: BoundRoleState) {
    boundRoleQueryStates.set(this, state);
    Object.freeze(this);
  }

  connects<Actual extends object, Mode extends QueryMatchMode>(
    player: BoundVar<Actual, Mode>
      & RoleBindingCompatibility<Actual, Mode, Player, SubtypeRoot>,
  ): Predicate {
    const own = boundRoleState(this);
    const target = boundVarState(player);
    requireSameProjection(own.projection, target.projection, "generated role player");
    if (setsDisjoint(own.acceptedPlayers, modelDomainTypeKeys(target))) {
      throw new TypeError("generated role does not accept this projected player type");
    }
    return new PredicateValue(own.handle.connects(target.binding), own.projection);
  }

  is<Actual extends object, Mode extends QueryMatchMode>(
    player: BoundVar<Actual, Mode>
      & RoleBindingCompatibility<Actual, Mode, Player, SubtypeRoot>,
  ): Predicate {
    return this.connects(player);
  }
}

class BoundVarValue<Model extends object, Mode extends QueryMatchMode>
  implements BoundVar<Model, Mode> {
  declare readonly [SELECTION_BRAND]: () => Model;
  declare readonly [BOUND_VAR_BRAND]: (model: Model) => readonly [Model, Mode];

  constructor(state: BoundVarState) {
    boundVarStates.set(this, state);
    selectionStates.set(this, state);
    Object.freeze(this);
  }

  field<Owner extends string, Attribute extends string, Value>(
    token: FieldToken<Owner, Attribute, Value>
      & (Owner extends ModelTypeKey<Model> ? object : never),
  ): BoundField<Value> {
    const own = boundVarState(this);
    const definition = fieldTokenStates.get(token);
    if (definition === undefined || definition.owner !== own.modelTypeKey) {
      throw new TypeError("generated field token owner does not match the bound model");
    }
    const attributeTypeKey = attributeModelTypeKey(definition.attribute);
    return new BoundFieldValue({
      handle: own.binding.fieldOwnedBy(own.projection.matchModelType(definition.owner), definition.name),
      projection: own.projection,
      attributeTypeKey,
    });
  }

  role<
    Owner extends string,
    Role extends string,
    Player extends object,
    SubtypeRoot extends object,
  >(
    token: RoleToken<Owner, Role, Player, SubtypeRoot>
      & (Owner extends ModelTypeKey<Model> ? object : never),
  ): BoundRole<Player, SubtypeRoot> {
    const own = boundVarState(this);
    const definition = roleTokenStates.get(token);
    if (definition === undefined || definition.owner !== own.modelTypeKey) {
      throw new TypeError("generated role token owner does not match the bound model");
    }
    return new BoundRoleValue<Player, SubtypeRoot>({
      handle: own.binding.roleOwnedBy(
        own.projection.matchModelType(definition.owner),
        roleIdentityLabel(definition.role),
      ),
      projection: own.projection,
      acceptedPlayers: new Set(definition.acceptedPlayers),
    });
  }

  iid(iid: string): Predicate {
    const own = boundVarState(this);
    return new PredicateValue(own.binding.iid(iid), own.projection);
  }

  iidIn(iids: readonly string[]): Predicate {
    const own = boundVarState(this);
    return new PredicateValue(own.binding.iidIn([...iids]), own.projection);
  }

  collect(): Collected<Model> {
    const own = boundVarState(this);
    return new CollectedValue({ ...own, handle: own.binding.collect(), collection: true });
  }
}

class CollectedValue<Model extends object> implements Collected<Model> {
  declare readonly [SELECTION_BRAND]: () => readonly Model[];

  constructor(state: SelectionState) {
    selectionStates.set(this, state);
    Object.freeze(this);
  }

  distinct(distinct = true): Collected<Model> {
    const own = selectionState(this);
    return new CollectedValue({ ...own, handle: own.handle.distinct(distinct) });
  }

  orderBy(order: QueryOrder): Collected<Model> {
    const own = selectionState(this);
    const term = orderState(order);
    requireSameProjection(own.projection, term.projection, "generated order");
    return new CollectedValue({ ...own, handle: own.handle.orderBy(term.handle) });
  }
}

interface QueryState {
  readonly handle: RuntimeProjectionMatchQuery;
  readonly projection: InstalledRuntimeProjection;
  readonly connection: RuntimeProjectionConnection | null;
}
const queryStates = new WeakMap<object, QueryState>();
const REMOTE_QUERY_SESSION = Symbol("typebridge.generated-remote-query-session");

/** Immutable generated-only direct query. */
export class Query<out Output> {
  private constructor(state: QueryState) {
    queryStates.set(this, state);
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: { readonly [Index in keyof Models]: BoundVar<Models[Index]> }
  ): Query<Output> {
    const state = queryState(this);
    let handle = state.handle;
    for (const binding of bindings) {
      const item = boundVarState(binding);
      requireSameProjection(state.projection, item.projection, "generated binding");
      handle = handle.addHidden(item.binding);
    }
    return createQuery({ ...state, handle });
  }

  where(...predicates: readonly [Predicate, ...Predicate[]]): Query<Output> {
    const state = queryState(this);
    let handle = state.handle;
    for (const predicate of predicates) {
      const item = predicateState(predicate);
      requireSameProjection(state.projection, item.projection, "generated predicate");
      handle = handle.wherePredicate(item.handle);
    }
    return createQuery({ ...state, handle });
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): Query<Output> {
    const state = queryState(this);
    const leftState = boundVarState(left);
    const rightState = boundVarState(right);
    requireSameProjection(state.projection, leftState.projection, "generated binding");
    requireSameProjection(state.projection, rightState.projection, "generated binding");
    return createQuery({
      ...state,
      handle: state.handle.allowCrossJoin(leftState.binding, rightState.binding),
    });
  }

  one(): Output {
    const state = queryState(this);
    const result = state.projection.executeRows(
      state.handle,
      directConnection(state),
      [],
      0n,
      1n,
      "exactly_one",
    );
    return materializeRows(state, result)[0] as Output;
  }

  first(options: FirstOptions = {}): Output | null {
    const state = queryState(this);
    const result = state.projection.executeRows(
      state.handle,
      directConnection(state),
      nativeOrders(options.orderBy ?? [], state.projection),
      0n,
      1n,
      "bounded_many",
    );
    return (materializeRows(state, result)[0] as Output | undefined) ?? null;
  }

  rows(options: RowsOptions): readonly Output[] {
    const state = queryState(this);
    const orders = nativeOrders(options.orderBy ?? [], state.projection);
    const result = state.projection.executeRows(
      state.handle,
      directConnection(state),
      orders,
      windowValue(options.offset ?? 0n, "offset"),
      windowValue(options.limit, "limit"),
      "bounded_many",
    );
    return Object.freeze(materializeRows(state, result)) as readonly Output[];
  }

  pageBy<Root extends object>(root: BoundVar<Root>, options: PageOptions): Page<Output> {
    const state = queryState(this);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated page root");
    const result = state.projection.executePage(
      state.handle,
      directConnection(state),
      rootState.binding,
      nativeOrders(options.orderBy ?? [], state.projection),
      windowValue(options.offset ?? 0n, "offset"),
      windowValue(options.limit, "limit"),
      options.includeTotal ?? false,
    );
    return Object.freeze({
      items: Object.freeze(materializePageRows(state, result)) as readonly Output[],
      offset: result.pageOffset(state.handle),
      limit: result.pageLimit(state.handle),
      total: result.pageTotal(state.handle),
    });
  }

  countBy<Root extends object>(root: BoundVar<Root>): bigint {
    const state = queryState(this);
    const item = boundVarState(root);
    requireSameProjection(state.projection, item.projection, "generated count root");
    return state.projection.executeCount(state.handle, directConnection(state), item.binding);
  }

  existsBy<Root extends object>(root: BoundVar<Root>): boolean {
    const state = queryState(this);
    const item = boundVarState(root);
    requireSameProjection(state.projection, item.projection, "generated exists root");
    return state.projection.executeExists(state.handle, directConnection(state), item.binding);
  }

  aggregate<
    Root extends object,
    const Terms extends readonly [Aggregate<unknown>, ...Aggregate<unknown>[]],
  >(
    root: BoundVar<Root>,
    terms: Terms & AtMostSixteen<Terms>,
  ): AggregateOutputs<Terms> {
    const state = queryState(this);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated aggregate root");
    const prepared = prepareAggregateTerms(terms, state.projection);
    const result = state.projection.executeReduce(
      state.handle,
      directConnection(state),
      rootState.binding,
      null,
      prepared.reducers,
      prepared.inputs,
    );
    return materializeUngroupedReduction(state, result, prepared.outputs) as AggregateOutputs<Terms>;
  }

  groupBy<Root extends object, Group extends object>(
    root: BoundVar<Root>,
    group: BoundVar<Group>,
  ): GroupedQuery<Group>;
  groupBy<Root extends object, Group extends object>(
    root: BoundVar<Root>,
    group: BoundField<Group>,
  ): GroupedQuery<Group>;
  groupBy<
    Root extends object,
    const Groups extends readonly [object, object, ...object[]],
  >(
    root: BoundVar<Root>,
    ...groups: AtMostSixteen<{
      readonly [Index in keyof Groups]: BoundField<Groups[Index]>;
    }>
  ): GroupedQuery<Groups>;
  groupBy<Root extends object>(
    root: BoundVar<Root>,
    ...groups: readonly object[]
  ): GroupedQuery<object> {
    const state = queryState(this);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated aggregate root");
    return createGroupedQuery<object>(
      createQuery(state),
      rootState.binding,
      runtimeReductionGroup(groups, state.projection),
    );
  }
}

type RuntimeReductionGroup =
  | Readonly<{ kind: "binding"; handle: RuntimeProjectionMatchBinding }>
  | Readonly<{
      kind: "field";
      handle: RuntimeProjectionMatchField;
      attributeTypeKey: string;
    }>
  | Readonly<{
      kind: "fields";
      handles: readonly RuntimeProjectionMatchField[];
      attributeTypeKeys: readonly string[];
    }>;

function runtimeReductionGroup(
  groups: readonly object[],
  projection: InstalledRuntimeProjection,
): RuntimeReductionGroup {
  if (groups.length === 0) {
    throw new TypeError("generated aggregate grouping requires at least one group");
  }
  if (groups.length > 16) {
    throw new RangeError("generated aggregate grouping supports at most sixteen fields");
  }
  const binding = groups.length === 1 ? boundVarStates.get(groups[0]!) : undefined;
  if (binding !== undefined) {
    requireSameProjection(projection, binding.projection, "generated aggregate group");
    return Object.freeze({ kind: "binding", handle: binding.binding });
  }
  const fields = groups.map((group) => {
    const field = boundFieldQueryStates.get(group);
    if (field === undefined) {
      throw new TypeError(
        "generated aggregate grouping requires one BoundVar or one or more BoundFields",
      );
    }
    requireSameProjection(projection, field.projection, "generated aggregate group field");
    return field;
  });
  if (fields.length === 1) {
    return Object.freeze({
      kind: "field",
      handle: fields[0]!.handle,
      attributeTypeKey: fields[0]!.attributeTypeKey,
    });
  }
  return Object.freeze({
    kind: "fields",
    handles: Object.freeze(fields.map((field) => field.handle)),
    attributeTypeKeys: Object.freeze(fields.map((field) => field.attributeTypeKey)),
  });
}

interface GroupedQueryState {
  readonly query: Query<unknown>;
  readonly root: RuntimeProjectionMatchBinding;
  readonly group: RuntimeReductionGroup;
}
const groupedQueryStates = new WeakMap<object, GroupedQueryState>();

/** Immutable generated direct grouped-reduction query. */
export class GroupedQuery<out Group extends object> {
  private constructor(state: GroupedQueryState) {
    groupedQueryStates.set(this, Object.freeze(state));
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: { readonly [Index in keyof Models]: BoundVar<Models[Index]> }
  ): GroupedQuery<Group> {
    const state = groupedQueryState(this);
    let query = state.query;
    for (const binding of bindings) {
      query = query.match(binding);
    }
    return createGroupedQuery(query, state.root, state.group);
  }

  where(...predicates: readonly [Predicate, ...Predicate[]]): GroupedQuery<Group> {
    const state = groupedQueryState(this);
    return createGroupedQuery(state.query.where(...predicates), state.root, state.group);
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): GroupedQuery<Group> {
    const state = groupedQueryState(this);
    return createGroupedQuery(
      state.query.allowCrossJoin(left, right),
      state.root,
      state.group,
    );
  }

  aggregate<const Terms extends readonly [Aggregate<unknown>, ...Aggregate<unknown>[]]>(
    terms: Terms & AtMostSixteen<Terms>,
  ): readonly (readonly [Group, AggregateOutputs<Terms>])[] {
    const grouped = groupedQueryState(this);
    const state = queryState(grouped.query);
    const prepared = prepareAggregateTerms(terms, state.projection);
    const result = grouped.group.kind === "binding"
      ? state.projection.executeReduce(
          state.handle,
          directConnection(state),
          grouped.root,
          grouped.group.handle,
          prepared.reducers,
          prepared.inputs,
        )
      : grouped.group.kind === "field"
        ? state.projection.executeReduceByField(
            state.handle,
            directConnection(state),
            grouped.root,
            grouped.group.handle,
            prepared.reducers,
            prepared.inputs,
          )
        : state.projection.executeReduceByFields(
            state.handle,
            directConnection(state),
            grouped.root,
            [...grouped.group.handles],
            prepared.reducers,
            prepared.inputs,
          );
    return materializeGroupedReduction(state, result, grouped.group, prepared.outputs) as readonly (
      readonly [Group, AggregateOutputs<Terms>]
    )[];
  }
}

/** Package-local generated query construction over exact generated tokens. */
export class QuerySession {
  readonly #connection: RuntimeProjectionConnection | null;
  readonly #projection: InstalledRuntimeProjection;
  readonly #session: RuntimeProjectionMatchSession;

  constructor(connection: RuntimeProjectionConnection);
  constructor(connection: RuntimeProjectionConnection | typeof REMOTE_QUERY_SESSION) {
    this.#projection = requireProjection();
    if (connection === REMOTE_QUERY_SESSION) {
      this.#connection = null;
    } else {
      this.#projection.assertConnection(connection);
      this.#connection = connection;
    }
    this.#session = this.#projection.matchSession();
    Object.freeze(this);
  }

  var<Id extends string, Model extends object>(
    model: QueryModelToken<Id, Model>,
    matchMode?: "exact",
  ): BoundVar<Model, "exact">;
  var<Id extends string, Model extends object>(
    model: QueryModelToken<Id, Model>,
    matchMode: "subtypes",
  ): BoundVar<Model, "subtypes">;
  var<Id extends string, Model extends object>(
    model: QueryModelToken<Id, Model>,
    matchMode: QueryMatchMode = "exact",
  ): BoundVar<Model, "exact"> | BoundVar<Model, "subtypes"> {
    if (matchMode !== "exact" && matchMode !== "subtypes") {
      throw new TypeError('generated query match mode must be "exact" or "subtypes"');
    }
    const entry = exactModelToken(model);
    const label = this.#projection.matchModelType(entry.token.typeKey);
    const binding = matchMode === "exact" ? this.#session.exact(label) : this.#session.subtypes(label);
    const state: BoundVarState = {
      binding,
      handle: binding.one(),
      projection: this.#projection,
      modelTypeKey: entry.token.typeKey,
      collection: false,
      matchMode,
    };
    return matchMode === "exact"
      ? new BoundVarValue<Model, "exact">(state)
      : new BoundVarValue<Model, "subtypes">(state);
  }

  exact<Id extends string, Model extends object>(
    model: QueryModelToken<Id, Model>,
  ): BoundVar<Model, "exact"> {
    return this.var(model, "exact");
  }

  subtypes<Id extends string, Model extends object>(
    model: QueryModelToken<Id, Model>,
  ): BoundVar<Model, "subtypes"> {
    return this.var(model, "subtypes");
  }

  reachable<
    Source extends object,
    SourceMode extends QueryMatchMode,
    Target extends object,
    TargetMode extends QueryMatchMode,
    RelationId extends string,
    Relation extends object,
    FromOwner extends string,
    FromRole extends string,
    FromPlayer extends object,
    FromSubtypeRoot extends object,
    ToOwner extends string,
    ToRole extends string,
    ToPlayer extends object,
    ToSubtypeRoot extends object,
  >(
    source: BoundVar<Source, SourceMode>
      & RoleBindingCompatibility<Source, SourceMode, FromPlayer, FromSubtypeRoot>,
    target: BoundVar<Target, TargetMode>
      & RoleBindingCompatibility<Target, TargetMode, ToPlayer, ToSubtypeRoot>,
    relation: QueryModelToken<RelationId, Relation>,
    roleFrom: RoleToken<FromOwner, FromRole, FromPlayer, FromSubtypeRoot>
      & (RelationId extends FromOwner ? object : never),
    roleTo: RoleToken<ToOwner, ToRole, ToPlayer, ToSubtypeRoot>
      & (RelationId extends ToOwner ? object : never),
    bounds: Readonly<{ minDepth: number; maxDepth: number }>,
  ): Predicate {
    const sourceState = boundVarState(source);
    const targetState = boundVarState(target);
    requireSameProjection(this.#projection, sourceState.projection, "generated reachable source");
    requireSameProjection(this.#projection, targetState.projection, "generated reachable target");
    const relationEntry = exactModelToken(relation);
    const from = exactRoleToken(roleFrom, relationEntry.token.typeKey, sourceState, "roleFrom");
    const to = exactRoleToken(roleTo, relationEntry.token.typeKey, targetState, "roleTo");
    return new PredicateValue(this.#session.reachable(
      this.#projection.matchModelType(relationEntry.token.typeKey),
      roleIdentityLabel(from.role),
      roleIdentityLabel(to.role),
      sourceState.binding,
      targetState.binding,
      reachabilityDepth(bounds.minDepth, "minDepth"),
      reachabilityDepth(bounds.maxDepth, "maxDepth"),
    ), this.#projection);
  }

  query<const Selections extends readonly [Selection<unknown>, ...Selection<unknown>[]]>(
    ...selections: Selections & AtMostSixteen<Selections>
  ): Query<PositionalOutput<Selections>> {
    const states = querySelections(selections, this.#projection);
    const shape = this.#session.positional(states.map((state) => state.handle));
    return createQuery({
      handle: this.#session.query(shape),
      projection: this.#projection,
      connection: this.#connection,
    });
  }

  queryNamed<const Shape extends Readonly<Record<string, Selection<unknown>>>>(
    selections: NamedSelectionInput<Shape>,
  ): Query<NamedOutput<Shape>> {
    const entries = Object.entries(selections) as [string, Selection<unknown>][];
    const states = querySelections(entries.map(([, selection]) => selection), this.#projection);
    const shape = this.#session.named(entries.map(([name]) => name), states.map((state) => state.handle));
    return createQuery({
      handle: this.#session.query(shape),
      projection: this.#projection,
      connection: this.#connection,
    });
  }
}

/** Explicit immutable budgets bound into every generated remote query. */
export interface RemoteQueryLimits extends RuntimeProjectionRemoteLimits {}

/** One caller-owned request/response exchange. No retry is performed. */
export type RemoteQueryExchange = RuntimeProjectionRemoteExchange;

interface RemoteQueryState {
  readonly direct: Query<unknown>;
  readonly remote: RuntimeProjectionRemote;
}
const remoteQueryStates = new WeakMap<object, RemoteQueryState>();

/** Immutable generated query composition over one caller-owned remote exchange. */
export class RemoteQuery<out Output> {
  private constructor(state: RemoteQueryState) {
    remoteQueryStates.set(this, state);
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: { readonly [Index in keyof Models]: BoundVar<Models[Index]> }
  ): RemoteQuery<Output> {
    const state = remoteQueryState<Output>(this);
    let direct = state.direct;
    for (const binding of bindings) {
      direct = direct.match(binding);
    }
    return createRemoteQuery(direct, state.remote);
  }

  where(...predicates: readonly [Predicate, ...Predicate[]]): RemoteQuery<Output> {
    const state = remoteQueryState<Output>(this);
    return createRemoteQuery(state.direct.where(...predicates), state.remote);
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): RemoteQuery<Output> {
    const state = remoteQueryState<Output>(this);
    return createRemoteQuery(state.direct.allowCrossJoin(left, right), state.remote);
  }

  async one(): Promise<Output> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const result = await remote.remote.rows(state.handle, [], 0n, 1n, "exactly_one");
    return materializeRows(state, result)[0] as Output;
  }

  async first(options: FirstOptions = {}): Promise<Output | null> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const result = await remote.remote.rows(
      state.handle,
      nativeOrders(options.orderBy ?? [], state.projection),
      0n,
      1n,
      "bounded_many",
    );
    return (materializeRows(state, result)[0] as Output | undefined) ?? null;
  }

  async rows(options: RowsOptions): Promise<readonly Output[]> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const result = await remote.remote.rows(
      state.handle,
      nativeOrders(options.orderBy ?? [], state.projection),
      windowValue(options.offset ?? 0n, "offset"),
      windowValue(options.limit, "limit"),
      "bounded_many",
    );
    return Object.freeze(materializeRows(state, result)) as readonly Output[];
  }

  async pageBy<Root extends object>(
    root: BoundVar<Root>,
    options: PageOptions,
  ): Promise<Page<Output>> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated page root");
    const result = await remote.remote.page(
      state.handle,
      rootState.binding,
      nativeOrders(options.orderBy ?? [], state.projection),
      windowValue(options.offset ?? 0n, "offset"),
      windowValue(options.limit, "limit"),
      options.includeTotal ?? false,
    );
    return Object.freeze({
      items: Object.freeze(materializePageRows(state, result)) as readonly Output[],
      offset: result.pageOffset(state.handle),
      limit: result.pageLimit(state.handle),
      total: result.pageTotal(state.handle),
    });
  }

  async countBy<Root extends object>(root: BoundVar<Root>): Promise<bigint> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const item = boundVarState(root);
    requireSameProjection(state.projection, item.projection, "generated count root");
    const result = await remote.remote.count(state.handle, item.binding);
    return result.countValue(state.handle);
  }

  async existsBy<Root extends object>(root: BoundVar<Root>): Promise<boolean> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const item = boundVarState(root);
    requireSameProjection(state.projection, item.projection, "generated exists root");
    const result = await remote.remote.exists(state.handle, item.binding);
    return result.existsValue(state.handle);
  }

  async aggregate<
    Root extends object,
    const Terms extends readonly [Aggregate<unknown>, ...Aggregate<unknown>[]],
  >(
    root: BoundVar<Root>,
    terms: Terms & AtMostSixteen<Terms>,
  ): Promise<AggregateOutputs<Terms>> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated aggregate root");
    const prepared = prepareAggregateTerms(terms, state.projection);
    const result = await remote.remote.reduce(
      state.handle,
      rootState.binding,
      null,
      prepared.reducers,
      prepared.inputs,
    );
    return materializeUngroupedReduction(state, result, prepared.outputs) as AggregateOutputs<Terms>;
  }

  groupBy<Root extends object, Group extends object>(
    root: BoundVar<Root>,
    group: BoundVar<Group>,
  ): RemoteGroupedQuery<Group>;
  groupBy<Root extends object, Group extends object>(
    root: BoundVar<Root>,
    group: BoundField<Group>,
  ): RemoteGroupedQuery<Group>;
  groupBy<
    Root extends object,
    const Groups extends readonly [object, object, ...object[]],
  >(
    root: BoundVar<Root>,
    ...groups: AtMostSixteen<{
      readonly [Index in keyof Groups]: BoundField<Groups[Index]>;
    }>
  ): RemoteGroupedQuery<Groups>;
  groupBy<Root extends object>(
    root: BoundVar<Root>,
    ...groups: readonly object[]
  ): RemoteGroupedQuery<object> {
    const remote = remoteQueryState<Output>(this);
    const state = queryState(remote.direct);
    const rootState = boundVarState(root);
    requireSameProjection(state.projection, rootState.projection, "generated aggregate root");
    return createRemoteGroupedQuery<object>(
      createRemoteQuery(createQuery(state), remote.remote),
      rootState.binding,
      runtimeReductionGroup(groups, state.projection),
    );
  }
}

interface RemoteGroupedQueryState {
  readonly query: RemoteQuery<unknown>;
  readonly root: RuntimeProjectionMatchBinding;
  readonly group: RuntimeReductionGroup;
}
const remoteGroupedQueryStates = new WeakMap<object, RemoteGroupedQueryState>();

/** Immutable generated remote grouped-reduction query. */
export class RemoteGroupedQuery<out Group extends object> {
  private constructor(state: RemoteGroupedQueryState) {
    remoteGroupedQueryStates.set(this, Object.freeze(state));
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: { readonly [Index in keyof Models]: BoundVar<Models[Index]> }
  ): RemoteGroupedQuery<Group> {
    const state = remoteGroupedQueryState(this);
    let query = state.query;
    for (const binding of bindings) {
      query = query.match(binding);
    }
    return createRemoteGroupedQuery(query, state.root, state.group);
  }

  where(...predicates: readonly [Predicate, ...Predicate[]]): RemoteGroupedQuery<Group> {
    const state = remoteGroupedQueryState(this);
    return createRemoteGroupedQuery(state.query.where(...predicates), state.root, state.group);
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): RemoteGroupedQuery<Group> {
    const state = remoteGroupedQueryState(this);
    return createRemoteGroupedQuery(
      state.query.allowCrossJoin(left, right),
      state.root,
      state.group,
    );
  }

  async aggregate<const Terms extends readonly [Aggregate<unknown>, ...Aggregate<unknown>[]]>(
    terms: Terms & AtMostSixteen<Terms>,
  ): Promise<readonly (readonly [Group, AggregateOutputs<Terms>])[]> {
    const grouped = remoteGroupedQueryState(this);
    const remote = remoteQueryState(grouped.query);
    const state = queryState(remote.direct);
    const prepared = prepareAggregateTerms(terms, state.projection);
    const result = grouped.group.kind === "binding"
      ? await remote.remote.reduce(
          state.handle,
          grouped.root,
          grouped.group.handle,
          prepared.reducers,
          prepared.inputs,
        )
      : grouped.group.kind === "field"
        ? await remote.remote.reduceByField(
            state.handle,
            grouped.root,
            grouped.group.handle,
            prepared.reducers,
            prepared.inputs,
          )
        : await remote.remote.reduceByFields(
            state.handle,
            grouped.root,
            [...grouped.group.handles],
            prepared.reducers,
            prepared.inputs,
          );
    return materializeGroupedReduction(state, result, grouped.group, prepared.outputs) as readonly (
      readonly [Group, AggregateOutputs<Terms>]
    )[];
  }
}

/** Generated-only remote query construction over exact package tokens. */
export class RemoteQuerySession {
  readonly #direct: QuerySession;
  readonly #remote: RuntimeProjectionRemote;

  readonly var: QuerySession["var"];
  readonly exact: QuerySession["exact"];
  readonly subtypes: QuerySession["subtypes"];
  readonly reachable: QuerySession["reachable"];

  constructor(
    advertisement: Uint8Array,
    exchange: RemoteQueryExchange,
    limits: RemoteQueryLimits,
  ) {
    const projection = requireProjection();
    this.#remote = projection.remote(
      requireQueryAuthority(),
      advertisement,
      exchange,
      limits,
    );
    const Constructor = QuerySession as unknown as new (
      token: typeof REMOTE_QUERY_SESSION,
    ) => QuerySession;
    this.#direct = new Constructor(REMOTE_QUERY_SESSION);
    this.var = this.#direct.var.bind(this.#direct) as QuerySession["var"];
    this.exact = this.#direct.exact.bind(this.#direct) as QuerySession["exact"];
    this.subtypes = this.#direct.subtypes.bind(this.#direct) as QuerySession["subtypes"];
    this.reachable = this.#direct.reachable.bind(this.#direct) as QuerySession["reachable"];
    Object.freeze(this);
  }

  query<const Selections extends readonly [Selection<unknown>, ...Selection<unknown>[]]>(
    ...selections: Selections & AtMostSixteen<Selections>
  ): RemoteQuery<PositionalOutput<Selections>> {
    const query = this.#direct.query.bind(this.#direct) as unknown as (
      ...values: readonly [Selection<unknown>, ...Selection<unknown>[]]
    ) => Query<PositionalOutput<Selections>>;
    return createRemoteQuery(
      query(...(selections as readonly [Selection<unknown>, ...Selection<unknown>[]])),
      this.#remote,
    );
  }

  queryNamed<const Shape extends Readonly<Record<string, Selection<unknown>>>>(
    selections: NamedSelectionInput<Shape>,
  ): RemoteQuery<NamedOutput<Shape>> {
    return createRemoteQuery(this.#direct.queryNamed(selections), this.#remote);
  }
}

function createRemoteQuery<Output>(
  direct: Query<Output>,
  remote: RuntimeProjectionRemote,
): RemoteQuery<Output> {
  const Constructor = RemoteQuery as unknown as new (
    state: RemoteQueryState,
  ) => RemoteQuery<Output>;
  return new Constructor({ direct, remote });
}

function remoteQueryState<Output>(query: RemoteQuery<Output>): Readonly<{
  direct: Query<Output>;
  remote: RuntimeProjectionRemote;
}> {
  const state = remoteQueryStates.get(query);
  if (state === undefined) {
    throw new TypeError("generated RemoteQuery has invalid lineage");
  }
  return state as Readonly<{
    direct: Query<Output>;
    remote: RuntimeProjectionRemote;
  }>;
}

function createQuery<Output>(state: QueryState): Query<Output> {
  const Constructor = Query as unknown as new (state: QueryState) => Query<Output>;
  return new Constructor(state);
}

function queryState(query: object): QueryState {
  const state = queryStates.get(query);
  if (state === undefined) throw new TypeError("generated Query has invalid lineage");
  return state;
}

function createGroupedQuery<Group extends object>(
  query: Query<unknown>,
  root: RuntimeProjectionMatchBinding,
  group: RuntimeReductionGroup,
): GroupedQuery<Group> {
  const Constructor = GroupedQuery as unknown as new (
    state: GroupedQueryState,
  ) => GroupedQuery<Group>;
  return new Constructor({ query, root, group });
}

function groupedQueryState(query: object): GroupedQueryState {
  const state = groupedQueryStates.get(query);
  if (state === undefined) throw new TypeError("generated GroupedQuery has invalid lineage");
  return state;
}

function createRemoteGroupedQuery<Group extends object>(
  query: RemoteQuery<unknown>,
  root: RuntimeProjectionMatchBinding,
  group: RuntimeReductionGroup,
): RemoteGroupedQuery<Group> {
  const Constructor = RemoteGroupedQuery as unknown as new (
    state: RemoteGroupedQueryState,
  ) => RemoteGroupedQuery<Group>;
  return new Constructor({ query, root, group });
}

function remoteGroupedQueryState(query: object): RemoteGroupedQueryState {
  const state = remoteGroupedQueryStates.get(query);
  if (state === undefined) throw new TypeError("generated RemoteGroupedQuery has invalid lineage");
  return state;
}

interface PreparedAggregateTerms {
  readonly reducers: RuntimeProjectionReduction[];
  readonly inputs: (RuntimeProjectionMatchField | null)[];
  readonly outputs: readonly ReductionOutputSpec[];
}

function prepareAggregateTerms(
  terms: readonly Aggregate<unknown>[],
  projection: InstalledRuntimeProjection,
): PreparedAggregateTerms {
  if (terms.length === 0 || terms.length > 16) {
    throw new RangeError("generated aggregate requires between one and sixteen terms");
  }
  const reducers: RuntimeProjectionReduction[] = [];
  const inputs: (RuntimeProjectionMatchField | null)[] = [];
  const outputs: ReductionOutputSpec[] = [];
  for (const term of terms) {
    const state = aggregateQueryStates.get(term as object);
    if (state === undefined) {
      throw new TypeError("generated aggregate term has invalid lineage");
    }
    if (state.projection !== null) {
      requireSameProjection(projection, state.projection, "generated aggregate field");
    }
    reducers.push(state.reducer);
    inputs.push(state.input);
    outputs.push(state.output);
  }
  return { reducers, inputs, outputs: Object.freeze(outputs) };
}

function directConnection(state: QueryState): RuntimeProjectionConnection {
  if (state.connection === null) {
    throw new TypeError("generated remote query cannot use a direct terminal");
  }
  return state.connection;
}

function selectionState(selection: object): SelectionState {
  const state = selectionStates.get(selection);
  if (state === undefined) throw new TypeError("generated selection has invalid lineage");
  return state;
}

function boundVarState(binding: object): BoundVarState {
  const state = boundVarStates.get(binding);
  if (state === undefined) throw new TypeError("generated binding has invalid lineage");
  return state;
}

function setsDisjoint(left: ReadonlySet<string>, right: ReadonlySet<string>): boolean {
  for (const value of left) {
    if (right.has(value)) return false;
  }
  return true;
}

function typeIdentityKey(value: unknown, context: string): string {
  assertRecord(value, context);
  const kind = value["kind"];
  const label = value["label"];
  if (typeof kind !== "string" || typeof label !== "string") {
    throw new TypeError(`${context} has no model kind and label`);
  }
  return JSON.stringify({ kind, label });
}

function nominalUpcastTypeKeys(entry: RuntimeModelEntry): ReadonlySet<string> {
  const metadata = entry.definition.metadata;
  assertRecord(metadata, "generated model metadata");
  const completeRead = metadata["complete_read"];
  assertRecord(completeRead, "generated complete-read metadata");
  const upcasts = completeRead["nominal_upcasts"];
  if (!Array.isArray(upcasts)) {
    throw new TypeError("generated complete-read metadata has no nominal upcasts");
  }
  return new Set(upcasts.map((identity) => typeIdentityKey(identity, "generated nominal upcast")));
}

function modelDomainTypeKeys(state: BoundVarState): ReadonlySet<string> {
  if (state.matchMode === "exact") return new Set([state.modelTypeKey]);
  const domain = new Set<string>();
  for (const [typeKey, entry] of runtimeModels) {
    if (typeKey === state.modelTypeKey || nominalUpcastTypeKeys(entry).has(state.modelTypeKey)) {
      domain.add(typeKey);
    }
  }
  return domain;
}

function boundFieldState(field: object): BoundFieldState {
  const state = boundFieldQueryStates.get(field);
  if (state === undefined) throw new TypeError("generated field has invalid lineage");
  return state;
}

function boundRoleState(role: object): BoundRoleState {
  const state = boundRoleQueryStates.get(role);
  if (state === undefined) throw new TypeError("generated role has invalid lineage");
  return state;
}

function predicateState(predicate: object): Readonly<{
  handle: RuntimeProjectionMatchPredicate;
  projection: InstalledRuntimeProjection;
}> {
  const state = predicateQueryStates.get(predicate);
  if (state === undefined) throw new TypeError("generated predicate has invalid lineage");
  return state;
}

function orderState(order: object): Readonly<{
  handle: RuntimeProjectionMatchOrder;
  projection: InstalledRuntimeProjection;
}> {
  const state = orderQueryStates.get(order);
  if (state === undefined) throw new TypeError("generated order has invalid lineage");
  return state;
}

function tokenForTypeKey(typeKey: string): RuntimeModelToken {
  const entry = runtimeModels.get(typeKey);
  if (entry === undefined) throw new TypeError("generated token identifies an unknown package model");
  return entry.token;
}

function attributeModelTypeKey(identity: string): string {
  if (runtimeModels.has(identity)) return identity;
  try {
    const label = JSON.parse(identity) as unknown;
    if (typeof label === "string") {
      const typeKey = JSON.stringify({ kind: "attribute", label });
      tokenForTypeKey(typeKey);
      return typeKey;
    }
  } catch {
    // Fall through to the exact package-model rejection below.
  }
  throw new TypeError("generated field token identifies an unknown package attribute model");
}

function roleIdentityLabel(identity: string): string {
  try {
    const parsed = JSON.parse(identity) as unknown;
    assertRecord(parsed, "generated role identity");
    if (typeof parsed["label"] === "string") return parsed["label"];
  } catch {
    // Some providers encode a role identity directly as its label.
  }
  return identity;
}

function exactRoleToken(
  token: object,
  owner: string,
  player: BoundVarState,
  context: string,
): RoleTokenDefinition<string, string> {
  const definition = roleTokenStates.get(token);
  if (definition === undefined || definition.owner !== owner) {
    throw new TypeError(`generated ${context} token has the wrong relation owner`);
  }
  if (setsDisjoint(new Set(definition.acceptedPlayers), modelDomainTypeKeys(player))) {
    throw new TypeError(`generated ${context} token does not accept its endpoint`);
  }
  return definition;
}

function reachabilityDepth(value: number, name: string): number {
  if (!Number.isInteger(value) || value < 0 || value > 255) {
    throw new RangeError(`${name} must be an integer from 0 through 255`);
  }
  return value;
}

function querySelections(
  selections: readonly Selection<unknown>[],
  projection: InstalledRuntimeProjection,
): readonly SelectionState[] {
  if (selections.length === 0) throw new TypeError("generated query requires at least one selection");
  if (selections.length > 16) throw new RangeError("generated query supports at most sixteen selections");
  return selections.map((selection) => {
    const state = selectionState(selection);
    requireSameProjection(projection, state.projection, "generated selection");
    return state;
  });
}

function windowValue(value: bigint, name: string): bigint {
  if (typeof value !== "bigint" || value < 0n || value > (1n << 64n) - 1n) {
    throw new RangeError(`${name} must be an unsigned 64-bit bigint`);
  }
  return value;
}

function nativeOrders(
  orders: readonly QueryOrder[],
  projection: InstalledRuntimeProjection,
): RuntimeProjectionMatchOrder[] {
  return orders.map((order) => {
    const state = orderState(order);
    requireSameProjection(projection, state.projection, "generated order");
    return state.handle;
  });
}

function materializeThing(state: QueryState, thing: RuntimeProjectionMatchThing): unknown {
  const encoded = state.projection.materializeMatchThingJson(thing);
  return hydrateProjectedValue(parseProjectedWire(JSON.parse(encoded) as unknown));
}

function materializeUngroupedReduction(
  state: QueryState,
  result: RuntimeProjectionMatchResult,
  outputs: readonly ReductionOutputSpec[],
): readonly unknown[] {
  if (result.reductionRowCount(state.handle) !== 1) {
    throw new TypeError("generated ungrouped aggregate did not return exactly one row");
  }
  return materializeReductionValues(state, result, 0, outputs);
}

function materializeGroupedReduction(
  state: QueryState,
  result: RuntimeProjectionMatchResult,
  group: RuntimeReductionGroup,
  outputs: readonly ReductionOutputSpec[],
): readonly (readonly [unknown, readonly unknown[]])[] {
  return Object.freeze(Array.from(
    { length: result.reductionRowCount(state.handle) },
    (_, rowIndex) => Object.freeze([
      group.kind === "binding"
        ? materializeThing(state, result.reductionGroup(state.handle, rowIndex))
        : group.kind === "field"
          ? materializeFieldGroup(state, result, rowIndex, group.attributeTypeKey)
          : materializeFieldGroups(state, result, rowIndex, group.attributeTypeKeys),
      materializeReductionValues(state, result, rowIndex, outputs),
    ] as const),
  ));
}

function materializeFieldGroup(
  state: QueryState,
  result: RuntimeProjectionMatchResult,
  rowIndex: number,
  attributeTypeKey: string,
): unknown {
  const parsed = JSON.parse(result.reductionGroupValueJson(state.handle, rowIndex)) as unknown;
  assertRecord(parsed, "native field group value");
  const valueType = parsed["valueType"];
  const value = parsed["value"];
  if (!isScalarValueType(valueType)
      || (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean")) {
    throw new TypeError("native field group value has an invalid scalar wire");
  }
  const entry = runtimeModels.get(attributeTypeKey);
  if (entry === undefined || entry.definition.valueType !== valueType) {
    throw new TypeError("native field group value differs from its generated attribute type");
  }
  return entry.token[HYDRATE_COMPLETE_BRAND](null, scalarFromWire({ valueType, value }));
}

function materializeFieldGroups(
  state: QueryState,
  result: RuntimeProjectionMatchResult,
  rowIndex: number,
  attributeTypeKeys: readonly string[],
): readonly unknown[] {
  const parsed = JSON.parse(result.reductionGroupValuesJson(state.handle, rowIndex)) as unknown;
  if (!Array.isArray(parsed) || parsed.length !== attributeTypeKeys.length) {
    throw new TypeError("native tuple field group has an invalid arity");
  }
  return Object.freeze(parsed.map((value, index) => {
    assertRecord(value, "native tuple field group value");
    const valueType = value["valueType"];
    const scalar = value["value"];
    if (!isScalarValueType(valueType)
        || (typeof scalar !== "string" && typeof scalar !== "number"
          && typeof scalar !== "boolean")) {
      throw new TypeError("native tuple field group value has an invalid scalar wire");
    }
    const entry = runtimeModels.get(attributeTypeKeys[index]!);
    if (entry === undefined || entry.definition.valueType !== valueType) {
      throw new TypeError("native tuple field group value differs from its generated attribute type");
    }
    return entry.token[HYDRATE_COMPLETE_BRAND](null, scalarFromWire({ valueType, value: scalar }));
  }));
}

function materializeReductionValues(
  state: QueryState,
  result: RuntimeProjectionMatchResult,
  rowIndex: number,
  outputs: readonly ReductionOutputSpec[],
): readonly unknown[] {
  if (result.reductionValueCount(state.handle, rowIndex) !== outputs.length) {
    throw new TypeError("generated aggregate result term count changed after validation");
  }
  return Object.freeze(outputs.map((output, valueIndex) => {
    const kind = result.reductionValueKind(state.handle, rowIndex, valueIndex);
    if (kind !== output.kind) {
      throw new TypeError("generated aggregate result kind changed after validation");
    }
    const value = output.kind === "count"
      ? result.reductionCountValue(state.handle, rowIndex, valueIndex)
      : output.kind === "long"
        ? result.reductionLongValue(state.handle, rowIndex, valueIndex)
        : result.reductionDoubleValue(state.handle, rowIndex, valueIndex);
    if (!output.optional && value === null) {
      throw new TypeError("generated aggregate returned a missing required value");
    }
    return value;
  }));
}

function materializeRows(state: QueryState, result: RuntimeProjectionMatchResult): unknown[] {
  const outputCount = result.outputSlotCount(state.handle);
  const names = result.outputNames(state.handle);
  const rows: unknown[] = [];
  for (let rowIndex = 0; rowIndex < result.rowCount(state.handle); rowIndex += 1) {
    if (result.slotCount(state.handle, rowIndex) !== outputCount) {
      throw new TypeError("generated query result slot count changed after validation");
    }
    const slots = Array.from({ length: outputCount }, (_, slotIndex) =>
      materializeThing(state, result.slotThing(state.handle, rowIndex, slotIndex))
    );
    rows.push(materializeOutput(slots, names));
  }
  return rows;
}

function materializePageRows(state: QueryState, result: RuntimeProjectionMatchResult): unknown[] {
  const outputCount = result.outputSlotCount(state.handle);
  const names = result.outputNames(state.handle);
  const rows: unknown[] = [];
  for (let entryIndex = 0; entryIndex < result.pageEntryCount(state.handle); entryIndex += 1) {
    if (result.pageSlotCount(state.handle, entryIndex) !== outputCount) {
      throw new TypeError("generated page result slot count changed after validation");
    }
    const slots: unknown[] = [];
    for (let slotIndex = 0; slotIndex < outputCount; slotIndex += 1) {
      const values = Array.from(
        { length: result.pageSlotValueCount(state.handle, entryIndex, slotIndex) },
        (_, valueIndex) => materializeThing(
          state,
          result.pageSlotThing(state.handle, entryIndex, slotIndex, valueIndex),
        ),
      );
      slots.push(result.outputSlotIsCollection(state.handle, slotIndex) ? Object.freeze(values) : values[0]);
    }
    rows.push(materializeOutput(slots, names));
  }
  return rows;
}

function materializeOutput(slots: readonly unknown[], names: readonly string[] | null): unknown {
  if (names !== null) {
    return Object.freeze(Object.fromEntries(names.map((name, index) => [name, slots[index]])));
  }
  return slots.length === 1 ? slots[0] : Object.freeze(slots);
}


export function defineStruct<Id extends string, Value, Input>(definition: {
  readonly id: Id;
  readonly fields: readonly StructFieldDefinition[];
  readonly metadata: unknown;
}): StructFactory<Id, Value, Input> {
  const factory = (input: unknown): Value => {
    assertRecord(input, "struct input");
    const fields = new Map(definition.fields.map((field) => [field.name, field]));
    for (const name of Object.keys(input)) {
      if (!fields.has(name)) {
        throw new TypeError(`struct received unknown field ${name}`);
      }
    }
    const result: Record<string | symbol, unknown> = { __typebridgeStruct: definition.id };
    for (const field of definition.fields) {
      if (!(field.name in input) && !field.optional) {
        throw new TypeError(`struct is missing field ${field.name}`);
      }
      result[field.name] = field.name in input ? input[field.name] : null;
    }
    Object.defineProperty(result, STRUCT_BRAND, { value: definition.id });
    return Object.freeze(result) as Value;
  };
  Object.defineProperties(factory, {
    id: { value: definition.id, enumerable: true },
    metadata: { value: definition.metadata, enumerable: true },
  });
  return Object.freeze(factory) as StructFactory<Id, Value, Input>;
}

export function definePlaysToken<Player extends string, Role extends string>(definition: {
  readonly player: Player;
  readonly role: Role;
  readonly name: string;
  readonly multiplicity: Multiplicity;
  readonly metadata: unknown;
}): PlaysToken<Player, Role> {
  return Object.freeze({ kind: "plays", ...definition }) as PlaysToken<Player, Role>;
}

export function defineFunctionToken<
  Id extends string,
  Arguments extends readonly unknown[],
  Result,
>(definition: {
  readonly id: Id;
  readonly name: string;
  readonly metadata: unknown;
}): FunctionToken<Id, Arguments, Result> {
  return Object.freeze({ kind: "function", ...definition }) as FunctionToken<Id, Arguments, Result>;
}
