/**
 * Typed `query-plan/v3` programs built from generated model tokens.
 *
 * A {@link PlanBuilder} declares things, field values, inputs and `let`
 * values, then the writes over them. {@link PlanBuilder.build} validates the
 * program against a schema authority and returns a {@link Plan}, which a
 * {@link StagedTransaction} runs at commit. The program is one pipeline: a
 * write plan that matches nothing writes nothing.
 *
 * Every call forwards to the one Rust builder the Rust, Python and C SDKs
 * use, so the same program gives the same canonical bytes in every
 * language:
 *
 * - Handles are numbered in creation order.
 * - `exact`, `subtypes`, `field`, `links`, `where` and `let` add to the
 *   plan's single leading match, in call order.
 * - Writes run in call order after the match. Consecutive writes of one
 *   kind (`update`, `insert`, `put`, `delete`) share one stage.
 * - Things and fields are named from their labels (`item`, `item_stock`),
 *   with `_2`, `_3`, ... on collision. `input` and `let` names must be
 *   fresh.
 *
 * Content errors (an invalid literal, a duplicate name, a field the thing
 * does not own) never throw from a builder call: the first one is thrown by
 * `build` as a `PlanError`. Handles belong to the builder that created
 * them; passing one to another builder throws a `TypeError` at once.
 *
 * @module
 */

import type { AttributeValue, PlanAuthority } from "./index.js";
import { Iid } from "./index.js";
import { loadNative } from "./native.js";
import type {
  NativePlan,
  NativeStagedPlan,
  NativePlanBuilder,
  NativePlanOperand,
} from "./native.js";
import { planAuthorityHandle, planNativeCall } from "./plan-internals.js";
import type { PlanAnswer } from "./staged-transaction.js";
import { Expr, Value } from "./plan-values.js";
import type { QueryLimits, InstalledRuntimeProjection, NativeAuthoredPlanResult } from "./runtime-projection.js";

/** Query policy chosen once when a recording is created. */
export interface PlanOptions { readonly queryLimits?: QueryLimits; }

/** A generated model token, such as `Item`: its canonical type identity. */
export interface PlanModelToken<Id extends string = string> {
  readonly typeKey: Id;
}

/** A generated field token, such as `Item.fields.stock`. */
export interface PlanFieldToken<Owner extends string = string> {
  readonly kind: "field";
  readonly owner: Owner;
  readonly metadata: unknown;
}

/** A generated role token, such as `Purchase.roles.buyer`. */
export interface PlanRoleToken<Owner extends string = string> {
  readonly kind: "role";
  readonly owner: Owner;
  readonly role: string;
}

/** The scalar domain of an input column. */
export type PlanValueType =
  | "iid"
  | "string"
  | "long"
  | "double"
  | "boolean"
  | "date"
  | "datetime"
  | "datetime_tz"
  | "decimal"
  | "duration";

/**
 * A literal: a `string`, a `bigint` (long) or a `boolean`, or a typed
 * value from `long()`, `double()`, `decimal()`, `date()` and the other
 * value helpers. Plain numbers are not literals; use `double()` or a
 * `bigint`.
 */
export type PlanLiteral = Iid | string | bigint | boolean | AttributeValue;

/** A scalar operand: a {@link PlanValue}, a {@link PlanInput}, or a literal. */
export type PlanOperand = PlanValue | PlanInput<unknown> | PlanLiteral;
type PlanHostTypes = {
  iid: Iid;
  string: string; long: bigint; double: number; boolean: boolean;
  date: Date; datetime: Date; datetime_tz: Date; decimal: string; duration: string;
};
type LegacyInputValue = Iid | string | bigint | number | boolean | AttributeValue;

type Comparator =
  | "equal"
  | "not_equal"
  | "less"
  | "less_or_equal"
  | "greater"
  | "greater_or_equal"
  | "contains"
  | "starts_with"
  | "ends_with"
  | "like";
type ArithOp = "add" | "sub" | "mul" | "div" | "mod" | "pow";
type Builtin = "abs" | "ceil" | "floor" | "round" | "min" | "max" | "length";

type ExprNode =
  | { readonly kind: "operand"; readonly operand: PlanOperand }
  | {
      readonly kind: "arith";
      readonly op: ArithOp;
      readonly lhs: ExprNode;
      readonly rhs: ExprNode;
    }
  | { readonly kind: "call"; readonly func: Builtin; readonly args: readonly ExprNode[] };

// Handle -> its builder and dense index. Kept outside the classes so the
// declarations expose no internals.
const handles = new WeakMap<object, { readonly builder: PlanBuilder; readonly index: number }>();
const inputTypes = new WeakMap<object, PlanValueType>();
const exprNodes = new WeakMap<PlanExpr, ExprNode>();
const conditions = new WeakMap<
  PlanCondition,
  readonly [PlanOperand, Comparator, PlanOperand]
>();
const inputValues = new WeakMap<object, readonly [PlanInput<unknown>, unknown | null]>();
const nativeBuilders = new WeakMap<PlanBuilder, NativePlanBuilder>();

declare const planThingModel: unique symbol;
declare const planHandleKind: unique symbol;

// Infers `Id` from the thing alone, so a token of another model is an error
// instead of widening `Id` to a union.
type Owned<Id> = [Id][Id extends unknown ? 0 : never];

function register<T extends object>(handle: T, builder: PlanBuilder, index: number): T {
  handles.set(handle, { builder, index });
  return Object.freeze(handle);
}

function node(value: PlanExpr | PlanOperand): ExprNode {
  if (value instanceof PlanExpr) {
    const tree = exprNodes.get(value);
    if (tree === undefined) {
      throw new TypeError("expression was not built by PlanExpr or a plan value");
    }
    return tree;
  }
  return { kind: "operand", operand: value };
}

function expr(tree: ExprNode): PlanExpr {
  const created = new PlanExpr();
  exprNodes.set(created, tree);
  return Object.freeze(created);
}

function arith(op: ArithOp, lhs: PlanExpr | PlanOperand, rhs: PlanExpr | PlanOperand): PlanExpr {
  return expr({ kind: "arith", op, lhs: node(lhs), rhs: node(rhs) });
}

/**
 * A value expression for {@link PlanBuilder.let}: arithmetic over values,
 * inputs and literals (`stock.sub(1n)`), or a builtin (`PlanExpr.abs(x)`).
 */
export class PlanExpr {
  /** @internal */
  constructor() {}

  /** `this + other`. */
  add(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("add", this, other);
  }
  /** `this - other`. */
  sub(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("sub", this, other);
  }
  /** `this * other`. */
  mul(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mul", this, other);
  }
  /** `this / other`, floating division. */
  div(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("div", this, other);
  }
  /** `this % other`. */
  mod(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mod", this, other);
  }

  /** `base` to the power `exponent`. */
  static pow(base: PlanExpr | PlanOperand, exponent: PlanExpr | PlanOperand): PlanExpr {
    return arith("pow", base, exponent);
  }
  /** Absolute value. */
  static abs(value: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "abs", args: [node(value)] });
  }
  /** Smallest integer not below a decimal or double. */
  static ceil(value: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "ceil", args: [node(value)] });
  }
  /** Largest integer not above a decimal or double. */
  static floor(value: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "floor", args: [node(value)] });
  }
  /** Nearest integer to a decimal or double. */
  static round(value: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "round", args: [node(value)] });
  }
  /** The smaller of two values of one numeric domain. */
  static min(left: PlanExpr | PlanOperand, right: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "min", args: [node(left), node(right)] });
  }
  /** The larger of two values of one numeric domain. */
  static max(left: PlanExpr | PlanOperand, right: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "max", args: [node(left), node(right)] });
  }
  /** The length of a string. */
  static length(value: PlanExpr | PlanOperand): PlanExpr {
    return expr({ kind: "call", func: "length", args: [node(value)] });
  }
}

/** A comparison between two operands, added with {@link PlanBuilder.where}. */
export class PlanCondition {
  declare readonly [planHandleKind]: "condition";
  /** @internal */
  constructor(left: PlanOperand, comparator: Comparator, right: PlanOperand) {
    conditions.set(this, [left, comparator, right]);
    Object.freeze(this);
  }
}

/** A matched or created thing of the model with type identity `Id`. */
export class PlanThing<Id extends string = string> {
  declare readonly [planThingModel]: Id;
  /** @internal */
  constructor() {}
}

/** A scalar value bound by {@link PlanBuilder.field} or {@link PlanBuilder.let}. */
export class PlanValue {
  /** @internal */
  constructor() {}

  /** Require this value to equal `other`. */
  eq(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "equal", other);
  }
  /** Require this value to differ from `other`. */
  ne(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "not_equal", other);
  }
  /** Require this value to be less than `other`. */
  lt(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "less", other);
  }
  /** Require this value to be at most `other`. */
  le(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "less_or_equal", other);
  }
  /** Require this value to be greater than `other`. */
  gt(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "greater", other);
  }
  /** Require this value to be at least `other`. */
  ge(other: PlanOperand): PlanCondition {
    return new PlanCondition(this, "greater_or_equal", other);
  }

  /** `this + other`. */
  add(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("add", this, other);
  }
  /** `this - other`. */
  sub(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("sub", this, other);
  }
  /** `this * other`. */
  mul(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mul", this, other);
  }
  /** `this / other`, floating division. */
  div(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("div", this, other);
  }
  /** `this % other`. */
  mod(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mod", this, other);
  }
}

/** One column's value in an input row; see {@link PlanBuilder.row}. */
export class PlanInputValue<T = unknown> {
  declare readonly [planHandleKind]: "input-value";
  /** @internal */
  constructor(input: PlanInput<T>, value: T | null) {
    inputValues.set(this, [input, value]);
    Object.freeze(this);
  }
}

/**
 * A typed input column; rows supply its values through
 * {@link PlanInput.value}. Many rows run the one plan over all of them.
 */
export class PlanInput<T = LegacyInputValue> {
  declare private readonly scalar: T;
  /** @internal */
  constructor() {}

  /** This column's value in one input row, of the column's value type. */
  value(value: T): PlanInputValue<T> {
    return new PlanInputValue(this, value);
  }
  /** An absent value for an optional column. */
  absent(): PlanInputValue<T> {
    return new PlanInputValue(this, null);
  }

  /** `this + other`. */
  add(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("add", this, other);
  }
  /** `this - other`. */
  sub(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("sub", this, other);
  }
  /** `this * other`. */
  mul(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mul", this, other);
  }
  /** `this / other`, floating division. */
  div(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("div", this, other);
  }
  /** `this % other`. */
  mod(other: PlanExpr | PlanOperand): PlanExpr {
    return arith("mod", this, other);
  }
}

/**
 * A thing being inserted or put; add its fields and players, then take its
 * handle with {@link PlanNewThing.thing}.
 */
export class PlanNewThing<Id extends string = string> {
  readonly #thing: PlanThing<Id>;

  /** @internal */
  constructor(thing: PlanThing<Id>) {
    this.#thing = thing;
    Object.freeze(this);
  }

  /** Give the new thing a field value. */
  field(token: PlanFieldToken<Owned<Id>>, value: PlanOperand): this {
    newThingBuilder(this.#thing).thingField(this.#thing, token, value);
    return this;
  }

  /** Give the new relation a role player. */
  player(role: PlanRoleToken<Owned<Id>>, player: PlanThing): this {
    newThingBuilder(this.#thing).thingPlayer(this.#thing, role, player);
    return this;
  }

  /** The new thing's handle. */
  thing(): PlanThing<Id> {
    return this.#thing;
  }
}

interface NewThingOps {
  thingField(thing: PlanThing, token: PlanFieldToken, value: PlanOperand): void;
  thingPlayer(thing: PlanThing, role: PlanRoleToken, player: PlanThing): void;
}

const newThingOps = new WeakMap<PlanBuilder, NewThingOps>();

function newThingBuilder(thing: PlanThing): NewThingOps {
  const entry = handles.get(thing);
  const ops = entry === undefined ? undefined : newThingOps.get(entry.builder);
  if (ops === undefined) {
    throw new TypeError("plan handle does not belong to a PlanBuilder");
  }
  return ops;
}

function fieldIdentity(token: PlanFieldToken): string {
  // The declaring ownership, as generated Rust and C field tokens carry it.
  const metadata = token.metadata as { readonly declaring_id?: unknown } | null;
  return typeof metadata === "object" && metadata !== null && metadata.declaring_id !== undefined
    ? JSON.stringify(metadata.declaring_id)
    : "";
}

function literalParts(value: PlanLiteral, valueType?: PlanValueType): [string, unknown] {
  if (valueType === "iid") {
    if (!(value instanceof Iid)) throw new TypeError("IID input columns require Iid values");
    return ["string", value.toString()];
  }
  if (value instanceof Iid) return ["string", value.toString()];
  if (typeof value === "object" && value !== null && "value_type" in value) {
    if (value.value_type === "long" && /^-?\d+$/.test(value.value)) {
      return ["long", BigInt(value.value)];
    }
    // Anything else, including a malformed long, is the native builder's
    // to reject at build.
    return [value.value_type, value.value];
  }
  if (valueType !== undefined) {
    return [valueType, value];
  }
  switch (typeof value) {
    case "bigint":
      return ["long", value];
    case "boolean":
      return ["boolean", value];
    case "string":
      return ["string", value];
    default:
      // Not a literal; the native builder records the host-type error.
      return ["long", value];
  }
}

/** One template column reads an earlier selected thing IID. */
export interface StagedResultRef {
  readonly input: PlanInput<Iid>;
  readonly sourcePlan: number;
  readonly sourceColumn: number;
  readonly expectedTypes: readonly PlanModelToken[];
}

/** An authored invocation template for StagedTransaction.stageStack. */
export class StagedPlan {
  readonly #native: NativeStagedPlan;
  /** @internal */
  constructor(native: NativeStagedPlan) { this.#native = native; Object.freeze(this); }
  /** @internal */
  nativeHandle(): NativeStagedPlan { return this.#native; }
}

/**
 * Builds one {@link Plan}. See the {@link "plan" | module documentation} for
 * the rules.
 */
export class PlanBuilder {
  readonly #native: NativePlanBuilder;

  constructor(options: PlanOptions = {}) {
    this.#native = planNativeCall(() => new (loadNative().NodePlanBuilder)(options.queryLimits?.nativeHandle()));
    nativeBuilders.set(this, this.#native);
    newThingOps.set(this, {
      thingField: (thing, token, value) => {
        const index = this.#index(thing);
        this.#native.thingField(index, fieldIdentity(token), this.#operand(value));
      },
      thingPlayer: (thing, role, player) => {
        this.#native.thingPlayer(this.#index(thing), role.role, this.#index(player));
      },
    });
    Object.freeze(this);
  }

  #index(handle: object): number {
    const entry = handles.get(handle);
    if (entry === undefined || entry.builder !== this) {
      throw new TypeError("plan handle does not belong to this PlanBuilder");
    }
    return entry.index;
  }

  #operand(operand: PlanOperand, valueType?: PlanValueType): NativePlanOperand {
    if (operand instanceof PlanValue) {
      return { kind: "binding", index: this.#index(operand) };
    }
    if (operand instanceof PlanInput) {
      return { kind: "input", index: this.#index(operand) };
    }
    return { kind: "literal", index: this.#literal(operand, valueType) };
  }

  #literal(value: PlanLiteral, valueType?: PlanValueType): number {
    const [type, scalar] = literalParts(value, valueType);
    return this.#native.literal(type, scalar);
  }

  #expr(tree: ExprNode): number {
    switch (tree.kind) {
      case "operand":
        return this.#native.exprOperand(this.#operand(tree.operand));
      case "arith":
        return this.#native.exprArith(tree.op, this.#expr(tree.lhs), this.#expr(tree.rhs));
      case "call":
        return this.#native.exprCall(tree.func, tree.args.map((arg) => this.#expr(arg)));
    }
  }

  #thing<Id extends string>(index: number): PlanThing<Id> {
    return register(new PlanThing<Id>(), this, index);
  }

  /** Match things of exactly `model`. */
  exact<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id> {
    return this.#thing(this.#native.exact(model.typeKey));
  }

  /** Match things of `model` or any of its subtypes. */
  subtypes<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id> {
    return this.#thing(this.#native.subtypes(model.typeKey));
  }

  /** Bind the value of `thing`'s field. Things without the field do not match. */
  field<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>): PlanValue {
    const owner = this.#index(thing);
    return register(new PlanValue(), this, this.#native.field(owner, fieldIdentity(token)));
  }

  /** @internal Generated field access uses the shared binding cache. */
  fieldValue(thing: PlanThing, token: PlanFieldToken): PlanValue {
    return register(new PlanValue(), this, this.#native.fieldValue(this.#index(thing), fieldIdentity(token)));
  }

  /** @internal Generated IID lookup. */
  byIid(thing: PlanThing, iid: Iid): void {
    if (!(iid instanceof Iid)) throw new TypeError("byIid requires an Iid value");
    this.#native.byIid(this.#index(thing), iid.toString());
  }
  /** Restrict a matched thing to an invocation IID column. */
  byIidInput(thing: PlanThing, column: PlanInput<Iid>): this {
    if (inputTypes.get(column) !== "iid") throw new TypeError("byIidInput requires an IID input column");
    this.#native.byIidInput(this.#index(thing), this.#index(column));
    return this;
  }

  /** @internal Add a player without replacing existing role players. */
  link(relation: PlanThing, role: PlanRoleToken, player: PlanThing): void {
    this.#native.link(this.#index(relation), role.role, this.#index(player));
  }

  /** @internal Lift a generated helper's caller value into given. */
  bindValue(thing: PlanThing, field: string, kind: PlanValueType, value: unknown): PlanInput {
    const input = new PlanInput();
    inputTypes.set(input, kind);
    return register(input, this, this.#native.bindValue(this.#index(thing), field, kind, value));
  }

  /** @internal Name a generated assignment through the shared core. */
  letValue(thing: PlanThing, field: string, value: PlanExpr): PlanValue {
    const tree = exprNodes.get(value);
    if (tree === undefined) throw new TypeError("expression was not recorded by this SDK");
    return register(new PlanValue(), this, this.#native.letValue(this.#index(thing), field, this.#expr(tree)));
  }

  /** Declare an input column named `name`; each {@link row} supplies a value. */
  input(name: string, valueType: "iid"): PlanInput<Iid>;
  input(name: string, valueType: "string"): PlanInput<string>;
  input(name: string, valueType: PlanValueType): PlanInput;
  input(name: string, valueType: PlanValueType): PlanInput {
    return this.#input(name, valueType, false);
  }

  /** Declare an optional input column; rows may give it {@link PlanInput.absent}. */
  optionalInput(name: string, valueType: "iid"): PlanInput<Iid>;
  optionalInput(name: string, valueType: "string"): PlanInput<string>;
  optionalInput(name: string, valueType: PlanValueType): PlanInput;
  optionalInput(name: string, valueType: PlanValueType): PlanInput {
    return this.#input(name, valueType, true);
  }

  /** @internal Strong input typing for explicit generated templates. */
  typedInput<K extends PlanValueType>(name: string, kind: K, optional: boolean): PlanInput<PlanHostTypes[K]> {
    return this.#input<PlanHostTypes[K]>(name, kind, optional);
  }

  #input<T = LegacyInputValue>(name: string, valueType: PlanValueType, optional: boolean): PlanInput<T> {
    const input = new PlanInput<T>();
    inputTypes.set(input, valueType);
    return register(input, this, this.#native.input(name, valueType === "iid" ? "string" : valueType, optional));
  }

  /** Require `condition` of every matched row. */
  where(condition: PlanCondition): this {
    const parts = conditions.get(condition);
    if (parts === undefined) {
      throw new TypeError("where requires a PlanCondition from a value comparison");
    }
    const [left, comparator, right] = parts;
    this.#native.compare(this.#operand(left), comparator, this.#operand(right));
    return this;
  }

  /** Require `relation` to link `player` in `role`. */
  links<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this {
    this.#native.links(this.#index(relation), role.role, this.#index(player));
    return this;
  }

  /** Bind `name` to `value`, evaluated for each matched row. */
  let(name: string, value: PlanExpr | PlanOperand): PlanValue {
    const index = this.#native.let(name, this.#expr(node(value)));
    return register(new PlanValue(), this, index);
  }

  /** Replace `thing`'s field value with `value`. */
  update<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>, value: PlanOperand): this {
    const owner = this.#index(thing);
    this.#native.update(owner, fieldIdentity(token), this.#operand(value));
    return this;
  }

  /** Make `player` the `role` player of `relation`. */
  updatePlayer<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this {
    this.#native.updatePlayer(this.#index(relation), role.role, this.#index(player));
    return this;
  }

  /** Create a thing of `model`; give it fields and players on the result. */
  insert<Id extends string>(model: PlanModelToken<Id>): PlanNewThing<Id> {
    return new PlanNewThing(this.#thing<Id>(this.#native.insert(model.typeKey)));
  }

  /**
   * Match a thing of `model` with the fields and players given on the
   * result, or create it.
   */
  put<Id extends string>(model: PlanModelToken<Id>): PlanNewThing<Id> {
    return new PlanNewThing(this.#thing<Id>(this.#native.put(model.typeKey)));
  }

  /** Delete `thing`. */
  delete(thing: PlanThing): this {
    this.#native.delete(this.#index(thing));
    return this;
  }

  /** Delete `thing`'s field, keeping the thing. */
  deleteField<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>): this {
    this.#native.deleteField(this.#index(thing), fieldIdentity(token));
    return this;
  }

  /** Remove `player` from `relation`'s `role`. */
  deletePlayer<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this {
    this.#native.deletePlayer(this.#index(relation), role.role, this.#index(player));
    return this;
  }

  /**
   * Add one input row. Every column appears at most once; a missing
   * optional column is absent.
   */
  row(values: readonly PlanInputValue[]): this {
    const columns: number[] = [];
    const literals: (number | null)[] = [];
    for (const value of values) {
      const parts = inputValues.get(value);
      if (parts === undefined) {
        throw new TypeError("row requires values from PlanInput.value or PlanInput.absent");
      }
      const [input, literal] = parts;
      columns.push(this.#index(input));
      literals.push(literal === null ? null : this.#literal(literal as PlanLiteral, inputTypes.get(input)));
    }
    this.#native.row(columns, literals);
    return this;
  }

  /**
   * Project `columns`, in order. A write plan then answers these rows from
   * the same pipeline instead of its matched count.
   */
  returning(columns: readonly (PlanThing | PlanValue)[]): this {
    this.#native.returning(columns.map((column) => this.#index(column)));
    return this;
  }

  /** Describe `thing` and its listed players; bare roles are required. */
  fetch<Id extends string>(thing: PlanThing<Id>, roles: ReadonlyArray<PlanRoleToken<Owned<Id>> | readonly [PlanRoleToken<Owned<Id>>, boolean]> = []): this {
    const lowered = roles.map((entry): readonly [PlanRoleToken<Owned<Id>>, boolean] =>
      "kind" in entry ? [entry, true] : entry);
    this.#native.fetch(this.#index(thing), lowered.map(([role]) => role.role), lowered.map(([, required]) => required));
    return this;
  }

  /** Answer the number of rows a read plan matches. */
  count(): this {
    this.#native.count();
    return this;
  }

  /** Answer whether a read plan matches any row. */
  exists(): this {
    this.#native.exists();
    return this;
  }

  /**
   * Validate the program against `authority`, a generated package's
   * `schemaAuthority()` or a `PlanAuthority` over declared-schema bytes.
   *
   * A plan with writes answers its matched row count, or the
   * {@link returning} rows. A read answers rows, or {@link count} /
   * {@link exists}. Throws a `PlanError` for the first invalid call, a
   * write plan that also asked for count or exists, or a plan the schema
   * rejects (for example a field the thing does not own, or a key update).
   */
  build(authority: PlanAuthority): Plan {
    const native = planAuthorityHandle(authority);
    if (native === undefined) {
      throw new TypeError("PlanBuilder.build requires a type-bridge PlanAuthority");
    }
    const plan = new Plan(planNativeCall(() => this.#native.build(native)));
    planBindings.set(plan, { builder: this, authority, encode: undefined });
    return plan;
  }
  /** Finalize templates with absent reference cells for stageStack. */
  buildStaged(authority: PlanAuthority, rows: readonly (readonly PlanInputValue<unknown>[])[], references: readonly StagedResultRef[] = []): StagedPlan {
    const native = planAuthorityHandle(authority);
    if (native === undefined) throw new TypeError("PlanBuilder.buildStaged requires a type-bridge PlanAuthority");
    const columns: number[][] = [];
    const values: unknown[][] = [];
    for (const row of rows) {
      const cells = row.map(cell => { const value = inputValues.get(cell); if (value === undefined) throw new TypeError("expected a PlanInputValue"); return value; });
      columns.push(cells.map(([input]) => this.#index(input)));
      values.push(cells.map(([input, value]) =>
        value !== null && inputTypes.get(input) === "iid" ? literalParts(value as PlanLiteral, "iid")[1] : value,
      ));
    }
    const refs = references.map(reference => {
      if (inputTypes.get(reference.input) !== "iid") throw new TypeError("staged result references require an IID input column");
      return { input: this.#index(reference.input), source_plan: reference.sourcePlan, source_column: reference.sourceColumn, expected_types: reference.expectedTypes.map(model => JSON.parse(model.typeKey) as unknown) };
    });
    return new StagedPlan(planNativeCall(() => this.#native.buildStaged(native, columns, values, JSON.stringify(refs))));
  }

}

const plans = new WeakMap<object, NativePlan>();
const decoders = new WeakMap<object, (answerJson: string, projection?: NativeAuthoredPlanResult) => unknown>();
const decoderFactories = new WeakMap<object, (plan: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult) => unknown>();
type ScalarEncoder = (kind: PlanValueType, value: unknown) => unknown;
const planBindings = new WeakMap<object, { builder: PlanBuilder; authority: PlanAuthority; encode: ScalarEncoder | undefined }>();

/** @internal The native program behind `plan`, for staging. */
export function nativePlan(plan: Plan<unknown>): NativePlan {
  const native = plans.get(plan);
  if (native === undefined) {
    throw new TypeError("stage requires a Plan built by PlanBuilder.build");
  }
  return native;
}

/** @internal Apply the typed projection chosen by a generated helper. */
export function decodePlan<R>(plan: Plan<R>, answerJson: string, projection?: NativeAuthoredPlanResult): R {
  const decoder = decoders.get(plan);
  return (decoder === undefined ? JSON.parse(answerJson) : decoder(answerJson, projection)) as R;
}

/**
 * A validated `query-plan/v3` program. Stage it on a
 * {@link StagedTransaction} from `Database.stagedTransaction()` or
 * `ProxyConnection.transaction()`.
 */
export interface PlanExplanation {
  /** Canonical plan-shape fingerprint; excludes transported caller values. */
  readonly fingerprint: { readonly algorithm: string; readonly canonicalization: string; readonly domain: string; readonly digest: string };
  readonly format: string;
  readonly required_capabilities: readonly string[];
  readonly operation: { readonly kind: "rows" | "count" | "exists" | "write"; readonly returns?: "rows" | "matched" };
  readonly bindings: readonly { readonly id: number; readonly variable: string; readonly possible_types: readonly { readonly kind: "entity" | "relation" | "attribute"; readonly label: string }[] }[];
  readonly inputs: readonly { readonly public_name: string; readonly value_type: PlanValueType; readonly optional: boolean }[];
  readonly given: { readonly row_count: number; readonly canonical_bytes: number };
  readonly statements: readonly string[];
  readonly read_statements: number;
  readonly write_statements: number;
}

export class Plan<R = PlanAnswer> {
  declare private readonly resultType: R;
  /** @internal */
  constructor(native?: NativePlan, decoder?: (answerJson: string) => R) {
    if (native !== undefined) plans.set(this, native);
    if (decoder !== undefined) decoders.set(this, decoder);
    Object.freeze(this);
  }

  /** Start an empty program. */
  static builder(): PlanBuilder {
    return new PlanBuilder();
  }

  /** Record generated helpers synchronously; awaited continuations need a new context. */
  static build<R>(body: (plan: PlanRecording) => Plan<R>, options?: PlanOptions): Plan<R>;
  static build(body: (plan: PlanRecording) => void, options?: PlanOptions): Plan<Matched>;
  static build<R>(body: (plan: PlanRecording) => Plan<R> | void, options: PlanOptions = {}): Plan<R> | Plan<Matched> {
    return record(new PlanRecording(options), body);
  }

  /** Declare reusable explicit input columns before recording a template. */
  static template(options: PlanOptions = {}): PlanTemplate { return new PlanTemplate(options); }

  /** The plan's canonical `query-plan/v3` bytes, identical in every SDK. */
  canonicalBytes(): Uint8Array {
    const native = nativePlan(this);
    return planNativeCall(() => native.canonicalBytes());
  }

  /** Exact executed statements. Given values are omitted; embedded plan constants remain. */
  explain(): PlanExplanation {
    return planNativeCall(() => JSON.parse(nativePlan(this).explainJson()) as PlanExplanation);
  }

  /** The plan fingerprint digest, hex encoded. */
  fingerprint(): string {
    const native = nativePlan(this);
    return planNativeCall(() => native.fingerprint());
  }

  /** Whether the plan writes, which makes an inferred transaction kind `write`. */
  writes(): boolean {
    return nativePlan(this).writes();
  }

  /** Bind explicit template columns to a fresh immutable invocation. */
  bind(rows: Iterable<Iterable<PlanInputValue>>): Plan<R> {
    const binding = planBindings.get(this);
    if (binding === undefined) throw new TypeError("build the template before binding rows");
    const columns: number[][] = [];
    const values: unknown[][] = [];
    for (const row of rows) {
      const indexes: number[] = [];
      const cells: unknown[] = [];
      for (const cell of row) {
        const entry = inputValues.get(cell);
        if (entry === undefined) throw new TypeError("a template row requires input cells");
        const [input, value] = entry;
        const handle = handles.get(input);
        if (handle === undefined || handle.builder !== binding.builder) throw new TypeError("input does not belong to this template");
        const kind = inputTypes.get(input)!;
        indexes.push(handle.index);
        cells.push(value === null ? null : binding.encode === undefined || kind === "iid" ? literalParts(value as PlanLiteral, kind)[1] : binding.encode(kind, value));
      }
      columns.push(indexes);
      values.push(cells);
    }
    const authority = planAuthorityHandle(binding.authority);
    if (authority === undefined) throw new TypeError("the template has no generated schema authority");
    const native = planNativeCall(() => nativePlan(this).bindRows(authority, columns, values));
    const result = new Plan<R>(native);
    const factory = decoderFactories.get(this);
    if (factory !== undefined) {
      decoderFactories.set(result, factory);
      decoders.set(result, (json, projection) => factory(native, json, projection));
    }
    planBindings.set(result, binding);
    return result;
  }
}

/** The count produced by an unprojected write. */
export class Matched {
  constructor(readonly count: bigint) { Object.freeze(this); }
}

function contextError(code: string, message: string): Error {
  return Object.assign(new Error(`${code}: ${message}`), { code, category: "invalid_contract", path: [] });
}

type ResultSelection = {
  readonly recording: PlanRecording;
  readonly kind: "matched" | "count" | "exists" | "models" | "rows";
  readonly thing?: number;
  readonly declared?: string;
  readonly roles?: readonly { role: string; required: boolean }[];
  readonly decode?: (plan: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult) => unknown;
  readonly columns?: readonly (PlanValue | PlanThing)[];
  readonly modelSlots?: readonly { readonly binding: number; readonly declared: string }[];
};
const selections = new WeakMap<object, ResultSelection>();
let currentRecording: PlanRecording | undefined;

function record<R>(recording: PlanRecording, body: (plan: PlanRecording) => Plan<R> | void): Plan<R> | Plan<Matched> {
  const previous = currentRecording;
  currentRecording = recording;
  try {
    const selected = body(recording);
    if (selected !== undefined && !(selected instanceof Plan)) throw contextError("plan_context_async", "Plan.build requires a synchronous callback; helpers after await need a new context");
    return recording.finish(selected);
  } finally {
    currentRecording = previous;
  }
}

/** A reusable plan with explicit input columns and rows. */
export class PlanTemplate {
  readonly #recording: PlanRecording;
  #built = false;
  constructor(options: PlanOptions = {}) { this.#recording = new PlanRecording(options); }
  private requireOpen(): void {
    if (this.#built) throw contextError("plan_already_built", "a template recording can be built once; bind it for each invocation");
  }
  input<K extends PlanValueType>(name: string, kind: K): PlanInput<PlanHostTypes[K]> { this.requireOpen(); return this.#recording.builder.typedInput(name, kind, false); }
  optionalInput<K extends PlanValueType>(name: string, kind: K): PlanInput<PlanHostTypes[K]> { this.requireOpen(); return this.#recording.builder.typedInput(name, kind, true); }
  row(cells: readonly PlanInputValue[]): this { this.requireOpen(); this.#recording.row(cells); return this; }
  build<R>(body: (plan: PlanRecording) => Plan<R>): Plan<R> {
    this.requireOpen();
    this.#built = true;
    return record(this.#recording, body) as Plan<R>;
  }
}

/** The synchronous recording supplied to Plan.build. */
export class PlanRecording {
  /** @internal */
  readonly builder: PlanBuilder;
  #authority: PlanAuthority | undefined;
  #projection: InstalledRuntimeProjection | undefined;
  #decodeRows: ((projection: NativeAuthoredPlanResult) => unknown[][]) | undefined;
  #decodeScalars: ((plan: NativePlan, answerJson: string) => unknown[][]) | undefined;
  #encodeScalar: ScalarEncoder | undefined;
  readonly #rows: (readonly PlanInputValue[])[] = [];
  constructor(options: PlanOptions = {}) { this.builder = new PlanBuilder(options); }

  /** @internal */
  static current(): PlanRecording {
    if (currentRecording === undefined) {
      throw contextError("plan_context_missing", "generated helpers require a synchronous Plan.build callback; helpers after await need a new context");
    }
    return currentRecording;
  }

  /** @internal One generated package supplies the authority for a recording. */
  use(authority: PlanAuthority, decodeScalars?: (plan: NativePlan, answerJson: string) => unknown[][], encodeScalar?: ScalarEncoder, projection?: InstalledRuntimeProjection, decodeRows?: (projection: NativeAuthoredPlanResult) => unknown[][]): void {
    if (this.#authority !== undefined && this.#authority !== authority) {
      throw contextError("plan_schema_mismatch", "one plan uses one generated schema package");
    }
    this.#authority = authority;
    if (projection !== undefined) this.#projection = projection;
    if (decodeRows !== undefined) this.#decodeRows = decodeRows;
    if (decodeScalars !== undefined) this.#decodeScalars = decodeScalars;
    if (encodeScalar !== undefined) this.#encodeScalar = encodeScalar;
  }

  /** @internal */
  result<R>(selection: Omit<ResultSelection, "recording">): Plan<R> {
    if (PlanRecording.current() !== this) throw contextError("plan_context_missing", "the recording is no longer open");
    const result = new Plan<R>();
    selections.set(result, { recording: this, ...selection });
    return result;
  }

  /** Answer the matched row count. */
  count(): Plan<bigint> { return this.result({ kind: "count" }); }

  /** Answer whether any row matches. */
  exists(): Plan<boolean> { return this.result({ kind: "exists" }); }

  /** Require conditions involving bound values from this recording. */
  require(...conditions: PlanCondition[]): void {
    if (PlanRecording.current() !== this) throw contextError("plan_context_missing", "the recording is no longer open");
    for (const condition of conditions) {
      if (fieldConditions.has(condition)) throw new TypeError("class field tokens need a handle's where()");
      this.builder.where(condition);
    }
  }

  /** Select model and scalar tuples in argument order. */
  returning<const Values extends readonly (Value<unknown> | Handle<unknown>)[]>(...values: Values): Plan<{ -readonly [K in keyof Values]: Values[K] extends Value<infer T> ? T : Values[K] extends Handle<infer M> ? M : never }[]> {
    const modelSlots: { binding: number; declared: string }[] = [];
    const columns = values.map((value) => {
      if (value instanceof Handle) {
        if (value.recording !== this) throw contextError("plan_context_missing", "the model belongs to another recording");
        modelSlots.push({ binding: handles.get(value.thing)!.index, declared: value.definition.model.typeKey });
        return value.thing;
      }
      return value.raw;
    });
    return this.result({ kind: "rows", columns, modelSlots, decode: (plan, json, projection) => {
      if (modelSlots.length !== 0) {
        if (projection === undefined || this.#decodeRows === undefined) throw new TypeError("model tuples require the core's committed projection proof");
        return this.#decodeRows(projection);
      }
      if (this.#decodeScalars === undefined) throw new TypeError("the generated package has no scalar result codec");
      return this.#decodeScalars(plan, json);
    } });
  }

  /** @internal Defer rows until the generated package supplies its host scalar codec. */
  row(cells: readonly PlanInputValue[]): void { this.#rows.push(cells); }

  /** @internal */
  finish<R>(selected: Plan<R> | void): Plan<R> | Plan<Matched> {
    if (this.#authority === undefined) throw contextError("plan_context_missing", "a plan needs a generated model's schema authority");
    for (const row of this.#rows) this.builder.row(row.map((cell) => {
      const entry = inputValues.get(cell);
      if (entry === undefined) throw new TypeError("a template row requires input cells");
      const [input, value] = entry;
      return value === null || this.#encodeScalar === undefined ? cell : new PlanInputValue(input, this.#encodeScalar(inputTypes.get(input)!, value) as PlanLiteral);
    }));
    const result = selected ?? new Plan<Matched>();
    const selection: ResultSelection | undefined = selected === undefined ? { recording: this, kind: "matched" } : selections.get(selected);
    if (selection === undefined || selection.recording !== this) throw contextError("plan_context_missing", "the result belongs to another recording");
    const native = nativeBuilders.get(this.builder)!;
    const roles = selection.roles ?? [];
    const columns = (selection.columns ?? []).map((column) => {
      const handle = handles.get(column);
      if (handle === undefined || handle.builder !== this.builder) throw new TypeError("foreign plan value");
      return handle.index;
    });
    const authority = planAuthorityHandle(this.#authority);
    if (authority === undefined) throw new TypeError("build requires a generated schema authority");
    const built = planNativeCall(() => native.buildSelected(authority, selection.kind, selection.thing ?? null, roles.map((role) => role.role), roles.map((role) => role.required), columns, this.#projection?.nativeHandle() ?? null, selection.declared ?? null, selection.modelSlots?.map((slot) => slot.binding) ?? null, selection.modelSlots?.map((slot) => slot.declared) ?? null));
    plans.set(result, built);
    const decode = (native: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult): unknown => {
      if (selection.decode !== undefined) return selection.decode(native, answerJson, projection);
      const answer = JSON.parse(answerJson) as PlanAnswer;
      if (selection.kind === "count" && answer.kind === "count") return BigInt(answer.value);
      if (selection.kind === "exists" && answer.kind === "exists") return answer.value;
      if (selection.kind === "matched" && (answer.kind === "count" || answer.kind === "matched")) return new Matched(BigInt(answer.value));
      throw new TypeError("the plan answer does not match its selected result");
    };
    decoders.set(result, (json, projection) => decode(built, json, projection));
    decoderFactories.set(result, decode);
    planBindings.set(result, { builder: this.builder, authority: this.#authority, encode: this.#encodeScalar });
    return result;
  }
}

/** @internal Generated model metadata and its existing scalar codec. */
export interface PlanModelAuthoring<M> {
  readonly model: PlanModelToken;
  readonly projection: () => InstalledRuntimeProjection;
  readonly decodeRows: (projection: NativeAuthoredPlanResult) => unknown[][];
  readonly authority: () => PlanAuthority;
  readonly isFieldToken: (token: object) => boolean;
  readonly fields: Readonly<Record<string, {
    readonly token: PlanFieldToken;
    readonly kind: PlanValueType;
    readonly site: string;
    readonly encode: (value: unknown) => unknown;
  }>>;
  readonly roles: readonly { readonly name: string; readonly token: PlanRoleToken; readonly role: string; readonly required: boolean }[];
  readonly decode: (plan: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult) => M[];
  readonly decodeScalars: (plan: NativePlan, answerJson: string) => unknown[][];
  readonly encodeScalar: ScalarEncoder;
}

/** A model bound inside a synchronous plan recording. */
export class Handle<M> {
  declare private readonly modelType: M;
  readonly #values = new Map<string, Value<unknown>>();
  /** @internal */
  constructor(
    readonly recording: PlanRecording,
    readonly thing: PlanThing,
    readonly definition: PlanModelAuthoring<M>,
  ) {}

  private open(): void {
    if (PlanRecording.current() !== this.recording) throw contextError("plan_context_missing", "the handle belongs to another recording");
  }

  /** @internal Generated properties supply their schema field name and scalar type. */
  field<T>(name: string): Value<T> {
    this.open();
    const field = this.definition.fields[name];
    if (field === undefined) throw new TypeError(`unknown plan field ${name}`);
    let value = this.#values.get(name);
    if (value === undefined) {
      value = new Value(this.recording.builder.fieldValue(this.thing, field.token), {
        bind: (host, floating) => {
          this.open();
          return this.recording.builder.bindValue(this.thing, field.site, floating ? "double" : field.kind, floating ? host : field.encode(host));
        },
      });
      this.#values.set(name, value);
    }
    return value as Value<T>;
  }

  where(...conditions: PlanCondition[]): this {
    this.open();
    for (const condition of conditions) {
      const deferred = fieldConditions.get(condition);
      if (deferred === undefined) this.recording.builder.where(condition);
      else {
        if (!this.definition.isFieldToken(deferred.token)) throw new TypeError("field token is not issued by this package");
        const entry = Object.entries(this.definition.fields).find(([, field]) => fieldIdentity(field.token) === fieldIdentity(deferred.token));
        if (entry === undefined) throw new TypeError("the field token does not belong to this model");
        if (deferred.tokenRight) {
          // The left value is already bound; the token binds after it.
          const left = (deferred.value as Value<unknown>).raw;
          this.recording.builder.where(new PlanCondition(left, deferred.op, this.field<unknown>(entry[0]).raw));
          continue;
        }
        const left = this.field<unknown>(entry[0]);
        const right = deferred.value instanceof Value ? deferred.value.raw
          : deferred.value instanceof PlanInput ? deferred.value
          : this.recording.builder.bindValue(this.thing, entry[1].site, entry[1].kind, entry[1].encode(deferred.value));
        this.recording.builder.where(new PlanCondition(left.raw, deferred.op, right));
      }
    }
    return this;
  }

  /** @internal Per-model set signatures restrict the available fields. */
  setFields(fields: Readonly<Record<string, unknown>>): Plan<M[]> {
    this.open();
    for (const [name, host] of Object.entries(fields)) {
      if (host === undefined) continue;
      const field = this.definition.fields[name];
      if (field === undefined) throw new TypeError(`unknown plan field ${name}`);
      const value = host instanceof Expr
        ? this.recording.builder.letValue(this.thing, field.site, host.raw)
        : host instanceof Value ? host.raw
        : host instanceof PlanInput ? host
        : this.recording.builder.bindValue(this.thing, field.site, field.kind, field.encode(host));
      this.recording.builder.update(this.thing, field.token, value);
    }
    return this.models();
  }

  unset(field: PlanFieldToken): Plan<M[]> {
    this.open();
    if (!this.definition.isFieldToken(field) || !Object.values(this.definition.fields).some((own) => fieldIdentity(own.token) === fieldIdentity(field))) throw new TypeError("the field token does not belong to this model");
    this.recording.builder.deleteField(this.thing, field);
    return this.models();
  }

  /** @internal Generated role signatures supply nominal player handles. */
  linkPlayers(players: Readonly<Record<string, Handle<unknown> | readonly Handle<unknown>[] | undefined>>, remove = false): Plan<M[]> {
    this.open();
    for (const [name, supplied] of Object.entries(players)) {
      if (supplied === undefined) continue;
      const role = this.definition.roles.find((role) => role.name === name);
      if (role === undefined) throw new TypeError(`unknown plan role ${name}`);
      const values = supplied instanceof Handle ? [supplied] : supplied;
      for (const player of values) {
        if (!(player instanceof Handle) || player.recording !== this.recording) throw new TypeError("foreign plan player");
        if (remove) this.recording.builder.deletePlayer(this.thing, role.token, player.thing);
        else this.recording.builder.link(this.thing, role.token, player.thing);
      }
    }
    return this.models();
  }

  delete(): Plan<Matched> {
    this.open();
    this.recording.builder.delete(this.thing);
    return this.recording.result({ kind: "matched" });
  }

  deleteReturning(): Plan<M[]> {
    this.open();
    this.recording.builder.delete(this.thing);
    return this.models();
  }

  /** @internal */
  models(): Plan<M[]> {
    const thing = handles.get(this.thing);
    if (thing === undefined || thing.builder !== this.recording.builder) throw new TypeError("foreign plan handle");
    return this.recording.result({ kind: "models", thing: thing.index, declared: this.definition.model.typeKey, roles: this.definition.roles, decode: this.definition.decode });
  }
}

/** @internal Shared recorder behind generated match/subtypes/byIid helpers. */
export function planMatch<M, H extends Handle<M>>(
  definition: PlanModelAuthoring<M>,
  HandleClass: new (recording: PlanRecording, thing: PlanThing, definition: PlanModelAuthoring<M>) => H,
  fields: Readonly<Record<string, unknown>> = {},
  options: { subtypes?: boolean; iid?: Iid; key?: readonly [string, unknown] } = {},
): H {
  const recording = PlanRecording.current();
  recording.use(definition.authority(), definition.decodeScalars, definition.encodeScalar, definition.projection(), definition.decodeRows);
  const thing = options.subtypes ? recording.builder.subtypes(definition.model) : recording.builder.exact(definition.model);
  const handle = new HandleClass(recording, thing, definition);
  if (options.iid !== undefined) recording.builder.byIid(thing, options.iid);
  if (options.key !== undefined) {
    const [name, host] = options.key;
    const field = definition.fields[name];
    if (field === undefined) throw new TypeError(`unknown plan key ${name}`);
    const bound = handle.field<unknown>(name);
    const value = host instanceof PlanInput ? host : recording.builder.bindValue(thing, "key", field.kind, field.encode(host));
    recording.builder.where(bound.raw.eq(value));
  }
  for (const [name, value] of Object.entries(fields)) {
    if (value !== undefined) recording.builder.where(handle.field<unknown>(name).eq(value));
  }
  return handle;
}

/** @internal Shared scalar insert/put recorder for generated factories. */
export function planInsert<M>(definition: PlanModelAuthoring<M>, fields: Readonly<Record<string, unknown>>, players: Readonly<Record<string, Handle<unknown> | readonly Handle<unknown>[] | undefined>> = {}, put = false): Plan<M[]> {
  const recording = PlanRecording.current();
  recording.use(definition.authority(), definition.decodeScalars, definition.encodeScalar, definition.projection(), definition.decodeRows);
  const created = put ? recording.builder.put(definition.model) : recording.builder.insert(definition.model);
  const thing = created.thing();
  for (const [name, host] of Object.entries(fields)) {
    if (host === undefined) continue;
    const field = definition.fields[name];
    if (field === undefined) throw new TypeError(`unknown plan field ${name}`);
    const value = host instanceof Expr ? recording.builder.letValue(thing, field.site, host.raw)
      : host instanceof Value ? host.raw : host instanceof PlanInput ? host
      : recording.builder.bindValue(thing, field.site, field.kind, field.encode(host));
    created.field(field.token, value);
  }
  for (const [name, supplied] of Object.entries(players)) {
    if (supplied === undefined) continue;
    const role = definition.roles.find((role) => role.name === name);
    if (role === undefined) throw new TypeError(`unknown plan role ${name}`);
    for (const player of supplied instanceof Handle ? [supplied] : supplied) {
      if (!(player instanceof Handle) || player.recording !== recording) throw new TypeError("foreign plan player");
      created.player(role.token, player.thing);
    }
  }
  return new Handle(recording, thing, definition).models();
}

/** @internal Core groups, splits and correlates generated entity Values rows. */
export function planInsertMany<M>(definition: PlanModelAuthoring<M>, rows: Iterable<object>, put = false): Plan<M[]> {
  const recording = PlanRecording.current();
  recording.use(definition.authority(), definition.decodeScalars, definition.encodeScalar, definition.projection(), definition.decodeRows);
  const created = put ? recording.builder.put(definition.model) : recording.builder.insert(definition.model);
  const thing = created.thing();
  const native = nativeBuilders.get(recording.builder)!;
  const fields = Object.entries(definition.fields);
  const columns = fields.map(([, field]) => ({ field: fieldIdentity(field.token), valueType: field.kind }));
  const lowered = Array.from(rows, (row) => {
    for (const name of Object.keys(row)) {
      if (definition.fields[name] === undefined) throw new TypeError(`unknown plan field ${name}`);
    }
    return fields.map(([name, field]) => {
      const value = (row as Readonly<Record<string, unknown>>)[name];
      return value === undefined ? undefined : field.encode(value);
    });
  });
  native.modelRows(handles.get(thing)!.index, columns, lowered);
  return new Handle(recording, thing, definition).models();
}

/** A generated class field, resolved against the handle passed to where(). */
export interface FieldToken<T> extends PlanFieldToken {
  eq(other: T | Value<T> | PlanInput<T>): PlanCondition;
  ne(other: T | Value<T> | PlanInput<T>): PlanCondition;
  lt(other: T | Value<T> | PlanInput<T>): PlanCondition;
  le(other: T | Value<T> | PlanInput<T>): PlanCondition;
  gt(other: T | Value<T> | PlanInput<T>): PlanCondition;
  ge(other: T | Value<T> | PlanInput<T>): PlanCondition;
  contains(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
  startsWith(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
  endsWith(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
  like(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
}

const fieldConditions = new WeakMap<PlanCondition, { token: PlanFieldToken; op: Comparator; value: unknown; tokenRight?: true }>();
const classFieldTokens = new WeakSet<object>();
const valueComparators = { eq: "equal", ne: "not_equal", lt: "less", le: "less_or_equal", gt: "greater", ge: "greater_or_equal" } as const;

/** @internal Whether `value` is a generated class field token. */
export function isClassFieldToken(value: unknown): value is PlanFieldToken {
  return typeof value === "object" && value !== null && classFieldTokens.has(value);
}

/** @internal `value op token`, with the token resolved by the handle receiving where(). */
export function valueFieldCondition(value: Value<unknown>, op: "eq" | "ne" | "lt" | "le" | "gt" | "ge", token: PlanFieldToken): PlanCondition {
  const result = new PlanCondition(new PlanValue(), valueComparators[op], false);
  fieldConditions.set(result, { token, op: valueComparators[op], value, tokenRight: true });
  return result;
}

/** @internal Decorate the generated runtime's existing nominal field token. */
export function planFieldToken<T, Token extends PlanFieldToken>(token: Token): Token & FieldToken<T> {
  const condition = (op: Comparator, value: unknown): PlanCondition => {
    const result = new PlanCondition(new PlanValue(), op, false);
    fieldConditions.set(result, { token, op, value });
    return result;
  };
  classFieldTokens.add(token);
  return Object.assign(token, {
    eq: (value: unknown) => condition("equal", value),
    ne: (value: unknown) => condition("not_equal", value),
    lt: (value: unknown) => condition("less", value),
    le: (value: unknown) => condition("less_or_equal", value),
    gt: (value: unknown) => condition("greater", value),
    ge: (value: unknown) => condition("greater_or_equal", value),
    contains: (value: unknown) => condition("contains", value),
    startsWith: (value: unknown) => condition("starts_with", value),
    endsWith: (value: unknown) => condition("ends_with", value),
    like: (value: unknown) => condition("like", value),
  });
}
