import type {
  EntityDescriptor,
  OwnedAttributeDescriptor,
  RelationDescriptor,
  RoleDescriptor,
  RuntimeAttributeValue,
  ValueType,
} from "../index.js";
import {
  hydrateAttributeEntries,
  runtimeAttributeValueFromUnknown,
} from "../codec.js";
import { setIid, type IidBearing } from "../iid.js";
import {
  FieldSpec,
  ListFieldSpec,
  RoleSpec,
  type SchemaSpec,
} from "../model.js";
import { loadNative } from "../native.js";
import { pageFromValidatedResult, type Page } from "./page.js";
import {
  TypedMatchError,
  nativeCall,
  type QueryModelClass,
} from "./references.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeSession = InstanceType<NativeModule["NodeMatchSessionHandle"]>;
type NativeQueryHandle = ReturnType<NativeSession["query"]>;
type NativeResultHandle = ReturnType<NativeQueryHandle["executeFetchRowsOwned"]>;
type NativeThingHandle = ReturnType<NativeResultHandle["slotThing"]>;

type AttributeFieldSpec = Extract<
  SchemaSpec,
  { readonly kind: "field" | "list-field" }
>;
type RelationRoleSpec = Extract<SchemaSpec, { readonly kind: "role" }>;
type ModelConstructor = QueryModelClass & {
  new (values: Record<string, unknown>): IidBearing;
};

interface FieldPlan {
  readonly fieldName: string;
  readonly spec: AttributeFieldSpec;
  readonly values: readonly RuntimeAttributeValue[];
}

interface RolePlan {
  readonly fieldName: string;
  readonly multiple: boolean;
  readonly players: readonly ThingPlan[];
}

interface ThingPlan {
  readonly typeName: string;
  readonly kind: "entity" | "relation";
  readonly parentType: string | null;
  readonly model: ModelConstructor;
  readonly iid: string;
  readonly fields: readonly FieldPlan[];
  readonly roles: readonly RolePlan[];
}

type ThingPosition = "selected" | "role-player";

interface OutputShapeProof {
  readonly names: readonly string[] | null;
  readonly collections: readonly boolean[];
}

/** @internal Materialize exactly one scalar/tuple/named row from a native proof. */
export function materializeValidatedOne(
  query: NativeQueryHandle,
  result: NativeResultHandle,
  models: ReadonlyMap<string, QueryModelClass>,
): unknown {
  const rows = materializeValidatedRows(query, result, models);
  if (rows.length !== 1) {
    throw resultDecode(
      "validated_one_row_count_mismatch",
      "exactly-one execution did not expose exactly one validated row",
    );
  }
  return rows[0];
}

/** @internal Materialize frozen scalar/tuple/named rows from one native proof. */
export function materializeValidatedRows(
  query: NativeQueryHandle,
  result: NativeResultHandle,
  models: ReadonlyMap<string, QueryModelClass>,
): readonly unknown[] {
  const rowCount = requireCount(
    nativeCall(() => result.rowCount(query)),
    "invalid_result_row_count",
    "validated result exposed an invalid row count",
  );
  const output = preflightOutputShape(query, result);
  if (output.collections.some(Boolean)) {
    throw resultDecode(
      "collection_in_fetch_rows_result",
      "FetchRows materialization cannot consume a collected output slot",
    );
  }

  const slotCounts: number[] = [];
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const slotCount = requireCount(
      nativeCall(() => result.slotCount(query, rowIndex)),
      "invalid_result_slot_count",
      "validated result exposed an invalid slot count",
    );
    if (slotCount !== output.collections.length) {
      throw resultDecode(
        "result_slot_count_mismatch",
        "validated row does not match its exact native output shape",
      );
    }
    slotCounts.push(slotCount);
  }

  // Inspect every opaque slot and nested role player before invoking any user
  // constructor. A hostile token, shape, descriptor, or hydration graph can
  // therefore never cause partial model construction.
  const plans: ThingPlan[][] = [];
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const row: ThingPlan[] = [];
    const slotCount = slotCounts[rowIndex]!;
    for (let slotIndex = 0; slotIndex < slotCount; slotIndex += 1) {
      const thing = nativeCall(() => result.slotThing(query, rowIndex, slotIndex));
      row.push(preflightThing(thing, models, "selected"));
    }
    plans.push(row);
  }

  const rows: unknown[] = [];
  for (const rowPlans of plans) {
    const slots = rowPlans.map(materializeThingPlan);
    if (output.names !== null) {
      rows.push(frozenNamedOutput(output.names, slots));
    } else if (slots.length === 1) {
      rows.push(slots[0]);
    } else {
      rows.push(Object.freeze(slots));
    }
  }
  return Object.freeze(rows);
}

/** @internal Materialize one immutable distinct-root page from a native proof. */
export function materializeValidatedPage(
  query: NativeQueryHandle,
  result: NativeResultHandle,
  models: ReadonlyMap<string, QueryModelClass>,
  expectedOffset: bigint,
  expectedLimit: bigint,
  includeTotal: boolean,
): Page<unknown> {
  const entryCount = requireCount(
    nativeCall(() => result.pageEntryCount(query)),
    "invalid_result_page_entry_count",
    "validated page exposed an invalid entry count",
  );
  const offset = requireU64BigInt(
    nativeCall(() => result.pageOffset(query)),
    "invalid_result_page_offset",
    "validated page exposed an invalid offset",
  );
  const limit = requireU64BigInt(
    nativeCall(() => result.pageLimit(query)),
    "invalid_result_page_limit",
    "validated page exposed an invalid limit",
    false,
  );
  if (offset !== expectedOffset || limit !== expectedLimit) {
    throw resultDecode(
      "result_page_window_mismatch",
      "validated page window differs from the exact terminal invocation",
    );
  }
  if (BigInt(entryCount) > limit) {
    throw resultDecode(
      "result_page_entry_count_mismatch",
      "validated page contains more entries than its exact limit",
    );
  }

  const rawTotal = nativeCall(() => result.pageTotal(query));
  const total = rawTotal === null
    ? undefined
    : requireU64BigInt(
        rawTotal,
        "invalid_result_page_total",
        "validated page exposed an invalid total",
      );
  if ((includeTotal && total === undefined) || (!includeTotal && total !== undefined)) {
    throw resultDecode(
      "result_page_total_presence_mismatch",
      "validated page total presence differs from the exact terminal invocation",
    );
  }
  if (total !== undefined) {
    const remaining = total > offset ? total - offset : 0n;
    const expectedEntries = remaining < limit ? remaining : limit;
    if (BigInt(entryCount) !== expectedEntries) {
      throw resultDecode(
        "result_page_total_mismatch",
        "validated page length is inconsistent with its same-snapshot total and window",
      );
    }
  }

  const output = preflightOutputShape(query, result);
  if (output.collections.filter((collection) => !collection).length !== 1) {
    throw resultDecode(
      "invalid_page_output_shape",
      "validated page output must contain exactly one singular root slot",
    );
  }

  const valueCounts: number[][] = [];
  for (let entryIndex = 0; entryIndex < entryCount; entryIndex += 1) {
    const slotCount = requireCount(
      nativeCall(() => result.pageSlotCount(query, entryIndex)),
      "invalid_result_slot_count",
      "validated page exposed an invalid slot count",
    );
    if (slotCount !== output.collections.length) {
      throw resultDecode(
        "result_slot_count_mismatch",
        "validated page row does not match its exact native output shape",
      );
    }
    const rowCounts: number[] = [];
    for (let slotIndex = 0; slotIndex < slotCount; slotIndex += 1) {
      const count = requireCount(
        nativeCall(() => result.pageSlotValueCount(query, entryIndex, slotIndex)),
        "invalid_result_slot_value_count",
        "validated page slot exposed an invalid value count",
      );
      if (!output.collections[slotIndex] && count !== 1) {
        throw resultDecode(
          "singular_page_slot_cardinality_mismatch",
          "validated singular page slot did not expose exactly one value",
        );
      }
      rowCounts.push(count);
    }
    valueCounts.push(rowCounts);
  }

  // As with FetchRows, complete the opaque graph proof before constructing a
  // single generated model or attribute instance.
  const plans: ThingPlan[][][] = [];
  for (let entryIndex = 0; entryIndex < entryCount; entryIndex += 1) {
    const row: ThingPlan[][] = [];
    for (let slotIndex = 0; slotIndex < output.collections.length; slotIndex += 1) {
      const slot: ThingPlan[] = [];
      const count = valueCounts[entryIndex]![slotIndex]!;
      for (let valueIndex = 0; valueIndex < count; valueIndex += 1) {
        const thing = nativeCall(() =>
          result.pageSlotThing(query, entryIndex, slotIndex, valueIndex),
        );
        slot.push(preflightThing(thing, models, "selected"));
      }
      row.push(slot);
    }
    plans.push(row);
  }

  const items: unknown[] = [];
  for (const rowPlans of plans) {
    const slots = rowPlans.map((slotPlans, slotIndex) => {
      const values = slotPlans.map(materializeThingPlan);
      return output.collections[slotIndex]
        ? Object.freeze(values)
        : values[0];
    });
    if (output.names !== null) {
      items.push(frozenNamedOutput(output.names, slots));
    } else if (slots.length === 1) {
      items.push(slots[0]);
    } else {
      items.push(Object.freeze(slots));
    }
  }
  return pageFromValidatedResult(items, offset, limit, total);
}

function frozenNamedOutput(
  names: readonly string[],
  values: readonly unknown[],
): Readonly<Record<string, unknown>> {
  const named: Record<string, unknown> = {};
  for (let index = 0; index < names.length; index += 1) {
    Object.defineProperty(named, names[index]!, {
      value: values[index],
      writable: false,
      enumerable: true,
      configurable: false,
    });
  }
  return Object.freeze(named);
}

/** @internal Preserve one lossless distinct-root count. */
export function materializeValidatedCount(
  query: NativeQueryHandle,
  result: NativeResultHandle,
): bigint {
  return requireU64BigInt(
    nativeCall(() => result.countValue(query)),
    "invalid_result_count",
    "validated count exposed an invalid lossless value",
  );
}

/** @internal Preserve one distinct-root existence proof. */
export function materializeValidatedExists(
  query: NativeQueryHandle,
  result: NativeResultHandle,
): boolean {
  const value = nativeCall(() => result.existsValue(query));
  if (typeof value !== "boolean") {
    throw resultDecode(
      "invalid_result_exists",
      "validated existence result did not expose a boolean",
    );
  }
  return value;
}

/** @internal Exact scalar proof expected for one typed reduction term. */
export interface ReductionOutputSpec {
  readonly kind: "count" | "long" | "double";
  readonly optional: boolean;
}

/**
 * @internal Decode one validated typed-reduction proof.
 *
 * Ungrouped results are returned as one frozen reducer tuple. Grouped results
 * are frozen rows of `[group, reducerTuple]`, in the canonical order proven by
 * Rust. Every scalar and group graph is preflighted before any user model
 * constructor runs.
 */
export function materializeValidatedReduction(
  query: NativeQueryHandle,
  result: NativeResultHandle,
  specs: readonly ReductionOutputSpec[],
  models: ReadonlyMap<string, QueryModelClass>,
  grouped: boolean,
): readonly unknown[] {
  const rowCount = requireCount(
    nativeCall(() => result.reductionRowCount(query)),
    "invalid_result_reduction_row_count",
    "validated reduction exposed an invalid row count",
  );
  if (!grouped && rowCount !== 1) {
    throw resultDecode(
      "result_reduction_row_count_mismatch",
      "ungrouped reduction must expose exactly one validated row",
    );
  }

  const rows: (readonly unknown[])[] = [];
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const valueCount = requireCount(
      nativeCall(() => result.reductionValueCount(query, rowIndex)),
      "invalid_result_reduction_value_count",
      "validated reduction exposed an invalid value count",
    );
    if (valueCount !== specs.length) {
      throw resultDecode(
        "result_reduction_value_count_mismatch",
        "validated reduction row does not match its exact reducer tuple",
      );
    }
    rows.push(Object.freeze(
      specs.map((spec, valueIndex) =>
        preflightReducedValue(query, result, rowIndex, valueIndex, spec),
      ),
    ));
  }

  if (!grouped) {
    return rows[0]!;
  }

  const groups: ThingPlan[] = [];
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const group = nativeCall(() => result.reductionGroup(query, rowIndex));
    groups.push(preflightThing(group, models, "selected"));
  }
  return Object.freeze(
    groups.map((group, rowIndex) =>
      Object.freeze([materializeThingPlan(group), rows[rowIndex]!] as const),
    ),
  );
}

function preflightReducedValue(
  query: NativeQueryHandle,
  result: NativeResultHandle,
  rowIndex: number,
  valueIndex: number,
  spec: ReductionOutputSpec,
): bigint | number | null {
  const kind = nativeCall(() =>
    result.reductionValueKind(query, rowIndex, valueIndex),
  );
  if (kind !== spec.kind) {
    throw resultDecode(
      "result_reduction_value_kind_mismatch",
      "validated reduction value differs from its requested output domain",
    );
  }
  let value: bigint | number | null;
  if (kind === "count") {
    value = requireU64BigInt(
      nativeCall(() =>
        result.reductionCountValue(query, rowIndex, valueIndex),
      ),
      "invalid_result_reduction_count",
      "validated reduction count is outside the lossless u64 domain",
    );
  } else if (kind === "long") {
    const raw = nativeCall(() =>
      result.reductionLongValue(query, rowIndex, valueIndex),
    );
    value = raw === null
      ? null
      : requireI64BigInt(
          raw,
          "invalid_result_reduction_long",
          "validated integer reduction is outside the lossless i64 domain",
        );
  } else {
    const raw = nativeCall(() =>
      result.reductionDoubleValue(query, rowIndex, valueIndex),
    );
    if (raw !== null && (typeof raw !== "number" || !Number.isFinite(raw))) {
      throw resultDecode(
        "invalid_result_reduction_double",
        "validated double reduction is not finite",
      );
    }
    value = raw;
  }
  if (value === null && !spec.optional) {
    throw resultDecode(
      "result_reduction_total_absent",
      "validated total reducer unexpectedly reported an absent value",
    );
  }
  return value;
}

function preflightOutputShape(
  query: NativeQueryHandle,
  result: NativeResultHandle,
): OutputShapeProof {
  const slotCount = requireCount(
    nativeCall(() => result.outputSlotCount(query)),
    "invalid_result_slot_count",
    "validated result exposed an invalid native output slot count",
  );
  if (slotCount === 0 || slotCount > 16) {
    throw resultDecode(
      "invalid_result_slot_count",
      "validated output must contain between one and sixteen slots",
    );
  }
  const rawNames = nativeCall(() => result.outputNames(query));
  const names = rawNames === null
    ? null
    : requireNames(rawNames, "output", "invalid_result_output_names");
  if (names !== null && names.length !== slotCount) {
    throw resultDecode(
      "named_result_slot_count_mismatch",
      "validated named output does not match its exact slot count",
    );
  }
  const collections: boolean[] = [];
  for (let slotIndex = 0; slotIndex < slotCount; slotIndex += 1) {
    const collection = nativeCall(() =>
      result.outputSlotIsCollection(query, slotIndex),
    );
    if (typeof collection !== "boolean") {
      throw resultDecode(
        "invalid_result_slot_kind",
        "validated output exposed an invalid slot kind",
      );
    }
    collections.push(collection);
  }
  return Object.freeze({
    names,
    collections: Object.freeze(collections),
  });
}

function preflightThing(
  thing: NativeThingHandle,
  models: ReadonlyMap<string, QueryModelClass>,
  position: ThingPosition,
): ThingPlan {
  const concreteDescriptor = nativeCall(() => thing.concreteDescriptor());
  const actualKind = nativeCall(() => thing.thingKind());
  if (typeof concreteDescriptor !== "string") {
    throw resultDecode(
      "invalid_result_descriptor",
      "validated thing did not expose a concrete descriptor identity",
    );
  }
  const prefix = `${actualKind}:`;
  if (!concreteDescriptor.startsWith(prefix) || concreteDescriptor.length === prefix.length) {
    throw resultDecode(
      "invalid_result_descriptor",
      "validated thing carries a malformed concrete descriptor identity",
    );
  }
  const typeName = concreteDescriptor.slice(prefix.length);
  if (typeName.includes(":")) {
    throw resultDecode(
      "invalid_result_descriptor",
      "validated thing carries a non-canonical concrete descriptor identity",
    );
  }

  // Exact concrete lookup is intentional: never substitute a declared base
  // constructor for a subtype returned by the provider.
  const registered = models.get(typeName);
  if (registered === undefined) {
    throw resultDecode(
      "unregistered_result_model",
      `validated concrete result type '${typeName}' has no registered model constructor`,
    );
  }
  const model = registered as ModelConstructor;
  const descriptor = modelDescriptor(model, typeName);
  const expectedKind = isRelationDescriptor(descriptor) ? "relation" : "entity";
  if (
    model.typeName !== typeName ||
    descriptor.type_name !== typeName ||
    actualKind !== expectedKind ||
    concreteDescriptor !== `${expectedKind}:${descriptor.type_name}`
  ) {
    throw resultDecode(
      "result_model_descriptor_mismatch",
      `validated concrete descriptor '${concreteDescriptor}' does not match its exact model constructor`,
    );
  }

  const iid = nativeCall(() => thing.iid());
  if (typeof iid !== "string" || iid.length === 0) {
    throw resultDecode(
      "invalid_result_iid",
      `validated result type '${typeName}' did not expose a non-empty IID`,
    );
  }

  const fields = preflightFields(thing, model, descriptor.owned_attributes);
  const roles = isRelationDescriptor(descriptor)
    ? position === "selected"
      ? preflightRoles(thing, model, descriptor.roles, models)
      : []
    : preflightEntityRoles(thing, typeName);
  return Object.freeze({
    typeName,
    kind: actualKind,
    parentType: descriptor.parent_type,
    model,
    iid,
    fields: Object.freeze(fields),
    roles: Object.freeze(roles),
  });
}

function preflightFields(
  thing: NativeThingHandle,
  model: ModelConstructor,
  descriptors: readonly OwnedAttributeDescriptor[],
): FieldPlan[] {
  const typeName = model.typeName;
  const nativeNames = requireNames(
    nativeCall(() => thing.fieldNames()),
    "field",
    "invalid_result_field_names",
  );
  const nativeNameSet = new Set(nativeNames);
  const descriptorByName = uniqueDescriptors(
    descriptors,
    (descriptor) => descriptor.field_name,
    "duplicate_result_model_field",
    `model constructor '${typeName}' has duplicate descriptor fields`,
  );

  for (const fieldName of nativeNames) {
    if (!descriptorByName.has(fieldName)) {
      throw resultDecode(
        "unknown_result_field",
        `validated concrete result exposes unknown field '${typeName}.${fieldName}'`,
      );
    }
  }

  for (const [fieldName, spec] of Object.entries(model.schema)) {
    if ((spec instanceof FieldSpec || spec instanceof ListFieldSpec) && !descriptorByName.has(fieldName)) {
      throw resultDecode(
        "result_model_descriptor_mismatch",
        `model field '${typeName}.${fieldName}' is absent from its concrete descriptor`,
      );
    }
  }

  const plans: FieldPlan[] = [];
  for (const descriptor of descriptors) {
    const fieldName = descriptor.field_name;
    const spec = model.schema[fieldName];
    if (!(spec instanceof FieldSpec || spec instanceof ListFieldSpec)) {
      throw resultDecode(
        "missing_result_model_field",
        `concrete descriptor field '${typeName}.${fieldName}' has no model field constructor`,
      );
    }
    if (
      spec.attrType.attrName !== descriptor.attr_name ||
      spec.attrType.valueType !== descriptor.value_type
    ) {
      throw resultDecode(
        "result_model_field_type_mismatch",
        `model field '${typeName}.${fieldName}' does not match its validated descriptor type`,
      );
    }

    const present = nativeNameSet.has(fieldName);
    if (!present) {
      if (minimumFieldCardinality(spec) > 0) {
        throw resultDecode(
          "missing_result_field",
          `validated result is missing required field '${typeName}.${fieldName}'`,
        );
      }
      continue;
    }
    const encoded = nativeCall(() => thing.fieldValuesJson(fieldName));
    if (encoded === null) {
      throw resultDecode(
        "missing_result_field_payload",
        `validated result listed field '${typeName}.${fieldName}' without its values`,
      );
    }
    const values = decodeFieldValues(encoded, descriptor.value_type, `${typeName}.${fieldName}`);
    requireFieldCardinality(spec, values.length, typeName, fieldName);
    if (spec instanceof FieldSpec && values.length === 0) {
      continue;
    }
    plans.push(Object.freeze({ fieldName, spec, values }));
  }
  return plans;
}

function preflightEntityRoles(
  thing: NativeThingHandle,
  typeName: string,
): RolePlan[] {
  const nativeNames = requireNames(
    nativeCall(() => thing.roleNames()),
    "role",
    "invalid_result_role_names",
  );
  if (nativeNames.length !== 0) {
    throw resultDecode(
      "entity_result_has_roles",
      `validated entity result '${typeName}' unexpectedly exposes relation roles`,
    );
  }
  return [];
}

function preflightRoles(
  thing: NativeThingHandle,
  model: ModelConstructor,
  descriptors: readonly RoleDescriptor[],
  models: ReadonlyMap<string, QueryModelClass>,
): RolePlan[] {
  const typeName = model.typeName;
  if (nativeCall(() => thing.roleDataComplete()) !== true) {
    throw resultDecode(
      "incomplete_relation_role_data",
      `validated relation result '${typeName}' does not carry complete root-role hydration`,
    );
  }
  const nativeNames = requireNames(
    nativeCall(() => thing.roleNames()),
    "role",
    "invalid_result_role_names",
  );
  const nativeNameSet = new Set(nativeNames);
  const descriptorByName = uniqueDescriptors(
    descriptors,
    (descriptor) => descriptor.role_name,
    "duplicate_result_model_role",
    `model constructor '${typeName}' has duplicate descriptor roles`,
  );
  for (const roleName of nativeNames) {
    if (!descriptorByName.has(roleName)) {
      throw resultDecode(
        "unknown_result_role",
        `validated relation result exposes unknown role '${typeName}.${roleName}'`,
      );
    }
  }

  const plans: RolePlan[] = [];
  for (const descriptor of descriptors) {
    const fieldName = descriptor.role_name;
    const spec = model.schema[fieldName];
    if (!(spec instanceof RoleSpec)) {
      throw resultDecode(
        "missing_result_model_role",
        `concrete descriptor role '${typeName}.${fieldName}' has no model role field`,
      );
    }
    requireRoleDescriptor(spec, descriptor, typeName);
    const count = requireCount(
      nativeCall(() => thing.rolePlayerCount(fieldName)),
      "invalid_result_role_player_count",
      `validated role '${typeName}.${fieldName}' exposed an invalid player count`,
    );
    if (!nativeNameSet.has(fieldName) && count !== 0) {
      throw resultDecode(
        "missing_result_role_payload",
        `validated relation omitted role '${typeName}.${fieldName}' while exposing players`,
      );
    }
    const [minimum, maximum] = descriptor.cardinality ?? [0, null];
    if (count < minimum || (maximum !== null && count > maximum)) {
      throw resultDecode(
        "result_role_cardinality_mismatch",
        `validated role '${typeName}.${fieldName}' violates its concrete descriptor cardinality`,
      );
    }

    const players: ThingPlan[] = [];
    for (let index = 0; index < count; index += 1) {
      const player = nativeCall(() => thing.rolePlayer(fieldName, index));
      const playerPlan = preflightThing(player, models, "role-player");
      if (!isAllowedRolePlayer(playerPlan, descriptor.player_type_names, models)) {
        throw resultDecode(
          "result_role_player_type_mismatch",
          `validated role '${typeName}.${fieldName}' contains an incompatible concrete player`,
        );
      }
      players.push(playerPlan);
    }
    const multiple =
      descriptor.cardinality !== null &&
      (descriptor.cardinality[1] === null || descriptor.cardinality[1] > 1);
    plans.push(Object.freeze({
      fieldName,
      multiple,
      players: Object.freeze(players),
    }));
  }
  return plans;
}

function isAllowedRolePlayer(
  player: ThingPlan,
  allowedTypes: readonly string[],
  models: ReadonlyMap<string, QueryModelClass>,
): boolean {
  let current: ThingPlan | null = player;
  const visited = new Set<string>();
  while (current !== null) {
    if (allowedTypes.includes(current.typeName)) {
      return true;
    }
    const parentType: string | null = current.parentType;
    if (parentType === null || visited.has(parentType)) {
      return false;
    }
    visited.add(parentType);
    const parent = models.get(parentType);
    if (parent === undefined) {
      return false;
    }
    const descriptor = modelDescriptor(parent as ModelConstructor, parentType);
    const kind = isRelationDescriptor(descriptor) ? "relation" : "entity";
    if (kind !== player.kind || descriptor.type_name !== parentType) {
      return false;
    }
    current = {
      ...player,
      typeName: parentType,
      parentType: descriptor.parent_type,
      kind,
      model: parent as ModelConstructor,
    };
  }
  return false;
}

function requireRoleDescriptor(
  spec: RelationRoleSpec,
  descriptor: RoleDescriptor,
  typeName: string,
): void {
  const players = spec.players.map((player) =>
    typeof player === "string" ? player : player.typeName,
  );
  if (
    players.length !== descriptor.player_type_names.length ||
    players.some((player, index) => player !== descriptor.player_type_names[index]) ||
    !equalCardinality(spec.cardinality, descriptor.cardinality)
  ) {
    throw resultDecode(
      "result_model_role_type_mismatch",
      `model role '${typeName}.${descriptor.role_name}' does not match its validated descriptor`,
    );
  }
}

function materializeThingPlan(plan: ThingPlan): IidBearing {
  const values: Record<string, unknown> = {};
  for (const field of plan.fields) {
    if (field.spec instanceof ListFieldSpec && field.values.length === 0) {
      values[field.fieldName] = Object.freeze([]);
      continue;
    }
    let hydrated: unknown;
    try {
      const entries = field.values.map(
        (value) => [field.spec.attrType.attrName, value] as const,
      );
      hydrated = hydrateAttributeEntries(entries, plan.model.schema)[field.fieldName];
    } catch {
      throw resultDecode(
        "result_attribute_construction_failed",
        `validated field '${plan.typeName}.${field.fieldName}' could not be materialized`,
      );
    }
    if (hydrated === undefined) {
      throw resultDecode(
        "result_field_hydration_failed",
        `validated field '${plan.typeName}.${field.fieldName}' could not be hydrated`,
      );
    }
    if (Array.isArray(hydrated)) {
      for (const attribute of hydrated) Object.freeze(attribute);
      values[field.fieldName] = Object.freeze(hydrated);
    } else {
      values[field.fieldName] = Object.freeze(hydrated);
    }
  }

  for (const role of plan.roles) {
    const players = role.players.map(materializeThingPlan);
    if (players.length === 0) {
      if (role.multiple) values[role.fieldName] = Object.freeze([]);
    } else {
      values[role.fieldName] = role.multiple || players.length > 1
        ? Object.freeze(players)
        : players[0];
    }
  }

  try {
    const instance = setIid(new plan.model(Object.freeze(values)), plan.iid);
    if (!(instance instanceof plan.model) || instance._iid !== plan.iid) {
      throw new TypeError("model constructor did not preserve its concrete identity");
    }
    return Object.freeze(instance);
  } catch {
    throw resultDecode(
      "result_model_construction_failed",
      `validated concrete result '${plan.typeName}' could not be constructed`,
    );
  }
}

function decodeFieldValues(
  encoded: string,
  valueType: ValueType,
  fieldName: string,
): readonly RuntimeAttributeValue[] {
  let decoded: unknown;
  try {
    decoded = JSON.parse(encoded) as unknown;
  } catch {
    throw resultDecode(
      "invalid_result_field_encoding",
      `validated field '${fieldName}' did not contain valid native JSON`,
    );
  }
  if (!Array.isArray(decoded)) {
    throw resultDecode(
      "invalid_result_field_encoding",
      `validated field '${fieldName}' did not contain an array of values`,
    );
  }
  try {
    return Object.freeze(decoded.map((value) => {
      requireExactValueTag(value, valueType);
      return runtimeAttributeValueFromUnknown(value, valueType);
    }));
  } catch {
    throw resultDecode(
      "invalid_result_attribute_value",
      `validated field '${fieldName}' did not match its concrete descriptor value type`,
    );
  }
}

const VALUE_TAGS: Readonly<Record<ValueType, string>> = Object.freeze({
  string: "String",
  long: "Long",
  double: "Double",
  boolean: "Boolean",
  date: "Date",
  datetime: "DateTime",
  "datetime-tz": "DateTimeTZ",
  decimal: "Decimal",
  duration: "Duration",
});

function requireExactValueTag(value: unknown, valueType: ValueType): void {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError("result attribute value is not tagged");
  }
  const record = value as Record<string, unknown>;
  const tag = VALUE_TAGS[valueType];
  const keys = Object.keys(record);
  if (keys.length !== 1 || keys[0] !== tag) {
    throw new TypeError("result attribute value tag does not match its descriptor");
  }
  const raw = record[tag];
  if (
    (valueType === "double" && (typeof raw !== "number" || !Number.isFinite(raw))) ||
    (valueType === "boolean" && typeof raw !== "boolean") ||
    (!matchesSpecialPrimitive(valueType) && typeof raw !== "string")
  ) {
    throw new TypeError("result attribute value payload has the wrong primitive type");
  }
  if (valueType === "long") {
    if (typeof raw !== "string" || !/^-?(0|[1-9][0-9]*)$/.test(raw)) {
      throw new TypeError("result long value is not canonical");
    }
    BigInt(raw);
  }
}

function matchesSpecialPrimitive(valueType: ValueType): boolean {
  return valueType === "double" || valueType === "boolean";
}

function requireFieldCardinality(
  spec: AttributeFieldSpec,
  actual: number,
  typeName: string,
  fieldName: string,
): void {
  const [minimum, maximum] = modelFieldCardinality(spec);
  if (actual < minimum || (maximum !== null && actual > maximum)) {
    throw resultDecode(
      "result_field_cardinality_mismatch",
      `validated field '${typeName}.${fieldName}' violates its model cardinality`,
    );
  }
}

function minimumFieldCardinality(spec: AttributeFieldSpec): number {
  return modelFieldCardinality(spec)[0];
}

function modelFieldCardinality(
  spec: AttributeFieldSpec,
): readonly [number, number | null] {
  if (spec instanceof ListFieldSpec) {
    return spec.card ?? [spec.isOptional ? 0 : 1, null];
  }
  return [spec.isOptional ? 0 : 1, 1];
}

function modelDescriptor(
  model: ModelConstructor,
  typeName: string,
): EntityDescriptor | RelationDescriptor {
  try {
    return model.descriptor();
  } catch {
    throw resultDecode(
      "result_model_descriptor_failed",
      `registered result model '${typeName}' could not expose its descriptor`,
    );
  }
}

function isRelationDescriptor(
  descriptor: EntityDescriptor | RelationDescriptor,
): descriptor is RelationDescriptor {
  return "roles" in descriptor;
}

function requireCount(value: unknown, code: string, message: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw resultDecode(code, message);
  }
  return value;
}

function requireI64BigInt(value: unknown, code: string, message: string): bigint {
  if (
    typeof value !== "bigint" ||
    value < -9_223_372_036_854_775_808n ||
    value > 9_223_372_036_854_775_807n
  ) {
    throw resultDecode(code, message);
  }
  return value;
}

const MAX_U64 = 18_446_744_073_709_551_615n;

function requireU64BigInt(
  value: unknown,
  code: string,
  message: string,
  allowZero = true,
): bigint {
  if (
    typeof value !== "bigint" ||
    value < 0n ||
    value > MAX_U64 ||
    (!allowZero && value === 0n)
  ) {
    throw resultDecode(code, message);
  }
  return value;
}

function requireNames(
  value: unknown,
  kind: "output" | "field" | "role",
  code: string,
): readonly string[] {
  if (!Array.isArray(value)) {
    throw resultDecode(code, `validated ${kind} names were not an array`);
  }
  const names: string[] = [];
  const seen = new Set<string>();
  for (const name of value) {
    if (typeof name !== "string" || name.length === 0 || seen.has(name)) {
      throw resultDecode(code, `validated ${kind} names were empty, duplicated, or malformed`);
    }
    seen.add(name);
    names.push(name);
  }
  return Object.freeze(names);
}

function uniqueDescriptors<Descriptor>(
  descriptors: readonly Descriptor[],
  name: (descriptor: Descriptor) => string,
  code: string,
  message: string,
): ReadonlyMap<string, Descriptor> {
  const result = new Map<string, Descriptor>();
  for (const descriptor of descriptors) {
    const descriptorName = name(descriptor);
    if (descriptorName.length === 0 || result.has(descriptorName)) {
      throw resultDecode(code, message);
    }
    result.set(descriptorName, descriptor);
  }
  return result;
}

function equalCardinality(
  left: readonly [number, number | null] | null,
  right: readonly [number, number | null] | null,
): boolean {
  return left === null
    ? right === null
    : right !== null && left[0] === right[0] && left[1] === right[1];
}

function resultDecode(code: string, message: string): TypedMatchError {
  return new TypedMatchError(
    "result_decode",
    code,
    message,
    Object.freeze([{ kind: "result" }]),
    Object.freeze({}),
  );
}
