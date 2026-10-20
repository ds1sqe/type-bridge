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
 * `build` as a `QueryV2Error`. Handles belong to the builder that created
 * them; passing one to another builder throws a `TypeError` at once.
 *
 * @module
 */

import type { AttributeValue, QueryV2Authority } from "./index.js";
import { loadNative } from "./native.js";
import type {
  NativePlan,
  NativePlanBuilder,
  NativePlanOperand,
} from "./native.js";
import { queryV2AuthorityHandle, queryV2NativeCall } from "./query-v2-internals.js";

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
export type PlanLiteral = string | bigint | boolean | AttributeValue;

/** A scalar operand: a {@link PlanValue}, a {@link PlanInput}, or a literal. */
export type PlanOperand = PlanValue | PlanInput | PlanLiteral;

type Comparator =
  | "equal"
  | "not_equal"
  | "less"
  | "less_or_equal"
  | "greater"
  | "greater_or_equal";
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
const inputTypes = new WeakMap<PlanInput, PlanValueType>();
const exprNodes = new WeakMap<PlanExpr, ExprNode>();
const conditions = new WeakMap<
  PlanCondition,
  readonly [PlanOperand, Comparator, PlanOperand]
>();
const inputValues = new WeakMap<PlanInputValue, readonly [PlanInput, PlanLiteral | null]>();

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
export class PlanInputValue {
  declare readonly [planHandleKind]: "input-value";
  /** @internal */
  constructor(input: PlanInput, value: PlanLiteral | null) {
    inputValues.set(this, [input, value]);
    Object.freeze(this);
  }
}

/**
 * A typed input column; rows supply its values through
 * {@link PlanInput.value}. Many rows run the one plan over all of them.
 */
export class PlanInput {
  /** @internal */
  constructor() {}

  /** This column's value in one input row, of the column's value type. */
  value(value: string | bigint | number | boolean | AttributeValue): PlanInputValue {
    return new PlanInputValue(this, value as PlanLiteral);
  }
  /** An absent value for an optional column. */
  absent(): PlanInputValue {
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

/**
 * Builds one {@link Plan}. See the {@link "plan" | module documentation} for
 * the rules.
 */
export class PlanBuilder {
  readonly #native: NativePlanBuilder;

  constructor() {
    this.#native = queryV2NativeCall(() => new (loadNative().NodePlanBuilder)());
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

  /** Declare an input column named `name`; each {@link row} supplies a value. */
  input(name: string, valueType: PlanValueType): PlanInput {
    return this.#input(name, valueType, false);
  }

  /** Declare an optional input column; rows may give it {@link PlanInput.absent}. */
  optionalInput(name: string, valueType: PlanValueType): PlanInput {
    return this.#input(name, valueType, true);
  }

  #input(name: string, valueType: PlanValueType, optional: boolean): PlanInput {
    const input = new PlanInput();
    inputTypes.set(input, valueType);
    return register(input, this, this.#native.input(name, valueType, optional));
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
      literals.push(literal === null ? null : this.#literal(literal, inputTypes.get(input)));
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
   * `schemaAuthority()` or a `QueryV2Authority` over declared-schema bytes.
   *
   * A plan with writes answers its matched row count, or the
   * {@link returning} rows. A read answers rows, or {@link count} /
   * {@link exists}. Throws a `QueryV2Error` for the first invalid call, a
   * write plan that also asked for count or exists, or a plan the schema
   * rejects (for example a field the thing does not own, or a key update).
   */
  build(authority: QueryV2Authority): Plan {
    const native = queryV2AuthorityHandle(authority);
    if (native === undefined) {
      throw new TypeError("PlanBuilder.build requires a type-bridge QueryV2Authority");
    }
    return new Plan(queryV2NativeCall(() => this.#native.build(native)));
  }
}

const plans = new WeakMap<Plan, NativePlan>();

/** @internal The native program behind `plan`, for staging. */
export function nativePlan(plan: Plan): NativePlan {
  const native = plans.get(plan);
  if (native === undefined) {
    throw new TypeError("stage requires a Plan built by PlanBuilder.build");
  }
  return native;
}

/**
 * A validated `query-plan/v3` program. Stage it on a
 * {@link StagedTransaction} from `RustDatabase.stagedTransaction()` or
 * `ProxyConnection.transaction()`.
 */
export class Plan {
  /** @internal */
  constructor(native: NativePlan) {
    plans.set(this, native);
    Object.freeze(this);
  }

  /** Start an empty program. */
  static builder(): PlanBuilder {
    return new PlanBuilder();
  }

  /** The plan's canonical `query-plan/v3` bytes, identical in every SDK. */
  canonicalBytes(): Uint8Array {
    const native = nativePlan(this);
    return queryV2NativeCall(() => native.canonicalBytes());
  }

  /** The plan fingerprint digest, hex encoded. */
  fingerprint(): string {
    const native = nativePlan(this);
    return queryV2NativeCall(() => native.fingerprint());
  }

  /** Whether the plan writes, which makes an inferred transaction kind `write`. */
  writes(): boolean {
    return nativePlan(this).writes();
  }
}
