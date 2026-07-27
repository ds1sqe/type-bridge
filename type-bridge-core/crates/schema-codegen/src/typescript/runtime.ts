import {
  installRuntimeProjection,
  type InstalledRuntimeProjection,
  type NativeProjectedManager,
  type RuntimeProjectionConnection,
} from "@type-bridge/node/runtime-projection";

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

export interface ProjectedModelManager<Complete> {
  insert(instance: Complete): Complete;
  getByIid(iid: string): Complete | null;
  all(): readonly Complete[];
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

export interface RoleToken<Owner extends string, Role extends string, Player> {
  readonly kind: "role";
  readonly owner: Owner;
  readonly role: Role;
  readonly name: string;
  readonly acceptedPlayers: readonly string[];
  readonly specializes: unknown | null;
  readonly multiplicity: Multiplicity;
  readonly abstract: boolean;
  readonly metadata: unknown;
  readonly [ROLE_TOKEN_BRAND]: (player: Player) => readonly [Owner, Role];
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
  | "date_time"
  | "date_time_tz"
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
let installedProjection: InstalledRuntimeProjection | null = null;

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
      case "date_time":
      case "date_time_tz":
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
  return Object.freeze({ kind: "field", ...definition }) as FieldToken<Owner, Attribute, Value>;
}

export function defineRoleToken<Owner extends string, Role extends string, Player>(
  definition: RoleTokenDefinition<Owner, Role>,
): RoleToken<Owner, Role, Player> {
  return Object.freeze({ kind: "role", ...definition }) as RoleToken<Owner, Role, Player>;
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

/** @internal Install exact native evidence after every generated model token is linked. */
export function __installRuntimeProjectionPackage(
  projectionJson: string,
  semanticFingerprintJson: string,
  projectionFingerprintJson: string,
  tokens: readonly object[],
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
  installedProjection = installRuntimeProjection({
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
}

function projectedManager<Complete>(
  typeKey: string,
  connection: RuntimeProjectionConnection,
): ProjectedModelManager<Complete> {
  const projection = installedProjection;
  if (projection === null) {
    throw new TypeError("generated runtime projection is not installed");
  }
  const native = projection.manager(typeKey, connection);
  return Object.freeze({
    insert(instance: Complete): Complete {
      const wire = lowerProjectedValue(instance);
      if (wire.typeKey !== typeKey || wire.form !== "complete") {
        throw new TypeError("projected insert requires the manager's exact complete model");
      }
      return hydrateNativeResult(native.insertJson(JSON.stringify(wire)), typeKey);
    },
    getByIid(iid: string): Complete | null {
      const value = JSON.parse(native.getByIidJson(iid)) as unknown;
      if (value === null) {
        return null;
      }
      return hydrateProjectedValue(parseProjectedWire(value), typeKey) as Complete;
    },
    all(): readonly Complete[] {
      const values = JSON.parse(native.allJson()) as unknown;
      if (!Array.isArray(values)) {
        throw new TypeError("native projected manager returned a non-sequence result");
      }
      return Object.freeze(values.map((value) => hydrateProjectedValue(parseProjectedWire(value), typeKey) as Complete));
    },
  });
}

function hydrateNativeResult<Complete>(json: string, expectedTypeKey: string): Complete {
  const value = JSON.parse(json) as unknown;
  return hydrateProjectedValue(parseProjectedWire(value), expectedTypeKey) as Complete;
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
    case "date_time":
      return { valueType, value: canonicalDateTime((value as Date).toISOString(), false) };
    case "date_time_tz":
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
    case "date_time":
      if (typeof wire.value !== "string") throw new TypeError("datetime wire requires a string");
      return new Date(`${wire.value}Z`);
    case "date_time_tz":
      if (typeof wire.value !== "string") throw new TypeError("datetime-tz wire requires a string");
      return new Date(wire.value);
    default:
      return wire.value;
  }
}

function isScalarValueType(value: unknown): value is ScalarValueType {
  return typeof value === "string" && [
    "string", "long", "double", "boolean", "date", "date_time", "date_time_tz", "decimal", "duration",
  ].includes(value);
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
